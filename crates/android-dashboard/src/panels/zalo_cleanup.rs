//! Explicit, device-bound cleanup of dated Zalo media; never clears app data.
use super::advanced_tools::{reports_dir, run_report};
use chrono::{FixedOffset, Months, NaiveDate, TimeZone, Utc};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const ROOT: &str = "/sdcard/Android/data/com.zing.zalo";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Filter {
    date: String,
    images: bool,
    videos: bool,
}
impl Default for Filter {
    fn default() -> Self {
        Self {
            date: Utc::now()
                .with_timezone(&zone())
                .date_naive()
                .checked_sub_months(Months::new(6))
                .unwrap()
                .to_string(),
            images: true,
            videos: true,
        }
    }
}
fn zone() -> FixedOffset {
    FixedOffset::east_opt(7 * 3600).unwrap()
}
fn cutoff(date: &str) -> Result<i64, String> {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| "Nhập ngày hợp lệ dạng YYYY-MM-DD.".to_string())?;
    if d.to_string() != date || d > Utc::now().with_timezone(&zone()).date_naive() {
        return Err("Mốc ngày không được nằm trong tương lai.".into());
    }
    Ok(zone()
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .timestamp())
}
#[derive(Clone, Debug)]
struct Media {
    path: String,
    modified: i64,
    bytes: u64,
}
#[derive(Clone)]
struct Preview {
    serial: String,
    adb: String,
    filter: Filter,
    root: String,
    files: Vec<Media>,
    scanned: usize,
}
enum Finished {
    Scan(Preview),
    Deleted(String),
}
#[derive(Default)]
pub struct ZaloCleanup {
    filter: Filter,
    preview: Option<Preview>,
    worker: Option<Receiver<Result<Finished, String>>>,
    message: String,
    confirmed: bool,
    progress: Option<Receiver<String>>,
    cancel: Arc<AtomicBool>,
    deleting: bool,
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}
fn media_kind(path: &str, filter: &Filter) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    (filter.images
        && matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "heic" | "heif" | "gif" | "jxl" | "avif" | "bmp"
        ))
        || (filter.videos
            && matches!(
                ext.as_str(),
                "mp4" | "mkv" | "mov" | "3gp" | "webm" | "avi" | "m4v"
            ))
}
fn safe_path(root: &str, path: &str) -> bool {
    path.strip_prefix(root)
        .is_some_and(|p| p.starts_with('/') && p.len() > 1)
        && !path.chars().any(char::is_control)
        && !path
            .split('/')
            .any(|p| p == ".." || p == "." || p.is_empty()) // leading slash handled below
}
fn valid_path(root: &str, path: &str) -> bool {
    path.starts_with('/') && safe_path(root.trim_start_matches('/'), path.trim_start_matches('/'))
}
fn report(adb: &str, serial: &str, script: String, name: &str) -> Result<String, String> {
    fs::create_dir_all(reports_dir()).map_err(|e| e.to_string())?;
    let path = reports_dir().join(format!(
        "zalo-{name}-{}-{}.txt",
        Utc::now().format("%Y%m%d-%H%M%S-%f"),
        std::process::id()
    ));
    run_report(
        adb,
        serial,
        &["shell".into(), script],
        Duration::from_secs(600),
        &path,
    )
    .map_err(|_| {
        format!(
            "Không hoàn tất thao tác {name}. Kiểm tra kết nối/quyền đọc và quét lại. Chi tiết: {}",
            path.display()
        )
    })?;
    fs::read_to_string(path).map_err(|e| e.to_string())
}
fn parse_inventory(text: &str, serial: &str, adb: &str, filter: Filter) -> Result<Preview, String> {
    let time = cutoff(&filter.date)?;
    let mut lines = text.lines();
    let root = lines
        .next()
        .and_then(|l| l.strip_prefix("ROOT|"))
        .ok_or("Không đọc được thư mục Zalo; kiểm tra quyền truy cập và hồ sơ người dùng.")?
        .to_string();
    if !root.starts_with("/storage/")
        || !root.ends_with("/Android/data/com.zing.zalo")
        || root.contains("..")
        || root.chars().any(char::is_control)
    {
        return Err("Thư mục Zalo không hợp lệ.".into());
    }
    let mut files = Vec::new();
    let mut scanned = 0;
    for line in lines {
        let mut parts = line.splitn(3, '|');
        let modified: i64 = parts
            .next()
            .unwrap_or("")
            .parse()
            .map_err(|_| "Không thể đọc đầy đủ ngày tệp; chưa cho phép xóa.")?;
        let bytes: u64 = parts
            .next()
            .unwrap_or("")
            .parse()
            .map_err(|_| "Không đọc được dung lượng tệp.")?;
        let path = parts.next().ok_or("Thiếu đường dẫn tệp.")?;
        if !valid_path(&root, path) {
            return Err("Có đường dẫn không an toàn; dừng quét.".into());
        }
        if media_kind(path, &filter) {
            scanned += 1;
            if modified > 0 && modified < time {
                files.push(Media {
                    path: path.into(),
                    modified,
                    bytes,
                });
            }
        }
    }
    files.sort_by_key(|f| f.modified);
    Ok(Preview {
        serial: serial.into(),
        adb: adb.into(),
        filter,
        root,
        files,
        scanned,
    })
}
fn scan(adb: &str, serial: &str, filter: Filter) -> Result<Preview, String> {
    // Stat all regular files in batches. Unknown extensions are counted neither as media nor deletion candidates.
    let script = format!(
        "r=$(readlink -f {}); [ -d \"$r\" ] && [ -r \"$r\" ] || exit 1; printf 'ROOT|%s\\n' \"$r\"; find \"$r\" -type f -print0 | xargs -0 -r -n 100 stat -c '%Y|%s|%n'",
        quote(ROOT)
    );
    parse_inventory(&report(adb, serial, script, "scan")?, serial, adb, filter)
}
fn delete_script(preview: &Preview, files: &[Media]) -> Result<String, String> {
    let time = cutoff(&preview.filter.date)?;
    let mut script = format!(
        "[ \"$(readlink -f {})\" = {} ] || exit 1;\n",
        quote(ROOT),
        quote(&preview.root)
    );
    for f in files {
        if !valid_path(&preview.root, &f.path)
            || !media_kind(&f.path, &preview.filter)
            || f.modified <= 0
            || f.modified >= time
        {
            return Err("Danh sách xóa không hợp lệ. Hãy quét lại.".into());
        }
        // Resolve each path again: symlink replacement, changed mtime/size and missing files are skipped.
        script.push_str(&format!("p={}; if [ ! -L \"$p\" ] && [ -f \"$p\" ] && [ \"$(readlink -f \"$p\")\" = \"$p\" ] && [ \"$(stat -c '%Y|%s' \"$p\" 2>/dev/null)\" = '{}' ]; then if rm -- \"$p\"; then echo DELETED; else echo FAILED; fi; else echo SKIPPED; fi\n", quote(&f.path), format_args!("{}|{}", f.modified, f.bytes)));
    }
    Ok(script)
}
fn delete(
    preview: Preview,
    progress: Sender<String>,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let (mut deleted, mut skipped, mut failed) = (0, 0, 0);
    for chunk in preview.files.chunks(20) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(format!(
                "Đã dừng trên {}: xóa {deleted} tệp, bỏ qua {skipped}, lỗi {failed}. Hãy quét lại trước khi tiếp tục.",
                preview.serial
            ));
        }
        let output = report(
            &preview.adb,
            &preview.serial,
            delete_script(&preview, chunk)?,
            "delete",
        )
        .map_err(|e| {
            format!("Đã xóa {deleted} tệp trước khi gặp lỗi. Quét lại trước khi tiếp tục. {e}")
        })?;
        let mut accounted = 0;
        for line in output.lines() {
            match line {
                "DELETED" => {
                    deleted += 1;
                    accounted += 1;
                }
                "SKIPPED" => {
                    skipped += 1;
                    accounted += 1;
                }
                "FAILED" => {
                    failed += 1;
                    accounted += 1;
                }
                _ => {}
            }
        }
        if accounted != chunk.len() {
            return Err(format!(
                "Kết quả không đầy đủ; đã xác nhận xóa {deleted} tệp. Hãy quét lại."
            ));
        }
        let _ = progress.send(format!(
            "{}: xử lý {}/{} tệp · đã xóa {deleted} · bỏ qua {skipped} · lỗi {failed}",
            preview.serial,
            deleted + skipped + failed,
            preview.files.len()
        ));
    }
    Ok(format!(
        "Thiết bị {}: đã xóa {deleted} tệp; bỏ qua {skipped} tệp thay đổi/không còn; lỗi {failed}. Tin nhắn và tài liệu được giữ nguyên.",
        preview.serial
    ))
}
impl ZaloCleanup {
    fn launch(
        &mut self,
        job: impl FnOnce(Sender<String>, Arc<AtomicBool>) -> Result<Finished, String> + Send + 'static,
    ) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.worker = Some(rx);
        self.confirmed = false;
        let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
        self.progress = Some(progress_rx);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job(progress_tx, cancel));
        });
    }
    pub fn show(&mut self, ui: &mut egui::Ui, serial: Option<&str>, adb: &str) {
        if let Some(progress) = &self.progress {
            if let Some(message) = progress.try_iter().last() {
                self.message = message;
            }
        }
        if let Some(rx) = &self.worker {
            match rx.try_recv() {
                Ok(result) => {
                    self.worker = None;
                    self.progress = None;
                    match result {
                        Ok(Finished::Scan(p))
                            if Some(p.serial.as_str()) == serial
                                && p.adb == adb
                                && p.filter == self.filter =>
                        {
                            self.message =
                                "Quét hoàn tất. Kiểm tra danh sách trước khi xóa.".into();
                            self.preview = Some(p);
                        }
                        Ok(Finished::Scan(_)) => {
                            self.message = "Thiết bị hoặc bộ lọc đã đổi. Hãy quét lại.".into()
                        }
                        Ok(Finished::Deleted(s)) => self.message = s,
                        Err(e) => self.message = e,
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.progress = None;
                    self.message = "Tác vụ bị ngắt. Hãy quét lại.".into();
                }
                _ => ui.ctx().request_repaint_after(Duration::from_millis(200)),
            }
        }
        if self.preview.as_ref().is_some_and(|p| {
            Some(p.serial.as_str()) != serial || p.adb != adb || p.filter != self.filter
        }) {
            self.preview = None;
            self.confirmed = false;
        }
        ui.collapsing("Dọn dữ liệu Zalo theo ngày", |ui| {
            ui.label("Giữ tin nhắn và tài liệu. Chỉ xóa ảnh/video cũ trong thư mục Zalo của hồ sơ hiện tại.");
            ui.small("Theo ngày sửa đổi tệp, không phải ngày gửi tin nhắn. Múi giờ UTC+7. Tệp không có đuôi nhận diện được sẽ được giữ lại.");
            let before = self.filter.clone();
            ui.add_enabled_ui(self.worker.is_none(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Giữ từ ngày (YYYY-MM-DD)");
                    ui.add(egui::TextEdit::singleline(&mut self.filter.date).desired_width(110.0));
                    if ui.button("Giữ 6 tháng gần nhất").clicked() { self.filter.date = Filter::default().date; }
                    ui.checkbox(&mut self.filter.images, "Ảnh"); ui.checkbox(&mut self.filter.videos, "Video");
                });
            });
            if before != self.filter { self.preview = None; self.confirmed = false; }
            let date_ok = cutoff(&self.filter.date);
            if let Err(e) = &date_ok { ui.colored_label(egui::Color32::YELLOW, e); }
            if ui.add_enabled(serial.is_some() && self.worker.is_none() && date_ok.is_ok() && (self.filter.images || self.filter.videos), egui::Button::new("Quét và xem trước")).clicked() {
                let (adb, serial, filter) = (adb.to_string(), serial.unwrap().to_string(), self.filter.clone());
                self.preview = None;
                self.message = format!("Đang quét thiết bị {serial}…");
                self.deleting = false;
                self.launch(move |_, _| scan(&adb, &serial, filter).map(Finished::Scan));
            }
            if self.worker.is_some() { ui.spinner(); ui.label("Đang xử lý; thư mục lớn có thể mất vài phút."); }
            if self.worker.is_some() && self.deleting {
                if ui.add_enabled(!self.cancel.load(Ordering::Relaxed), egui::Button::new("Dừng sau đợt hiện tại")).clicked() { self.cancel.store(true, Ordering::Relaxed); }
                if self.cancel.load(Ordering::Relaxed) { ui.label("Đang dừng; các tệp đã xóa không được khôi phục."); }
            }
            if let Some(p) = &self.preview {
                let bytes: u64 = p.files.iter().map(|f| f.bytes).sum();
                ui.strong(format!("{}: {} tệp có thể xóa / {} tệp ảnh-video đã quét · {:.2} GB", p.serial, p.files.len(), p.scanned, bytes as f64 / 1_073_741_824.0));
                if p.files.is_empty() { ui.label("Không có ảnh/video cũ hơn mốc đã chọn."); }
                egui::ScrollArea::vertical().id_salt("zalo_preview").max_height(220.0).show_rows(ui, 22.0, p.files.len(), |ui, range| {
                    for f in &p.files[range] {
                        let date = chrono::DateTime::from_timestamp(f.modified, 0).map(|d| d.with_timezone(&zone()).format("%d/%m/%Y").to_string()).unwrap_or_default();
                        ui.horizontal(|ui| { ui.label(date); ui.label(format!("{:.1} MB", f.bytes as f64 / 1_048_576.0)); ui.add(egui::Label::new(f.path.strip_prefix(&p.root).unwrap_or(&f.path)).truncate()).on_hover_text(&f.path); });
                    }
                });
                ui.small("Ảnh/video đã xóa có thể không tải lại được từ tin nhắn. Không có thùng rác.");
                ui.checkbox(&mut self.confirmed, "Tôi xác nhận xóa các tệp cũ trong danh sách trên");
                if ui.add_enabled(self.confirmed && !p.files.is_empty() && self.worker.is_none(), egui::Button::new("Xóa tệp đã xem trước")).clicked() {
                    let p = self.preview.take().unwrap();
                    self.message = format!("Đang xóa tệp cũ trên {}…", p.serial);
                    self.deleting = true;
                    self.launch(move |progress, cancel| delete(p, progress, cancel).map(Finished::Deleted));
                }
            }
            if !self.message.is_empty() { ui.label(&self.message); }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn filter() -> Filter {
        Filter {
            date: "2026-05-17".into(),
            images: true,
            videos: true,
        }
    }
    #[test]
    fn cutoff_boundary_and_types() {
        let t = cutoff("2026-05-17").unwrap();
        let r = "/storage/emulated/0/Android/data/com.zing.zalo";
        let text = format!(
            "ROOT|{r}\n{}|50|{r}/old.JPG\n{t}|60|{r}/keep.mp4\n{}|70|{r}/messages.db\n",
            t - 1,
            t - 1
        );
        let p = parse_inventory(&text, "phone-a", "adb", filter()).unwrap();
        assert_eq!(p.scanned, 2);
        assert_eq!(p.files.len(), 1);
        assert!(p.files[0].path.ends_with("old.JPG"));
        assert!(cutoff("2026-02-30").is_err());
        assert!(media_kind("photo.JXL", &filter()));
    }
    #[test]
    fn rejects_escape_and_partial_scan() {
        let r = "/storage/emulated/0/Android/data/com.zing.zalo";
        for path in [
            format!("{r}/../other/a.jpg"),
            format!("{r}-other/a.jpg"),
            "/data/data/com.zing.zalo/a.jpg".into(),
        ] {
            assert!(!valid_path(r, &path));
        }
        assert!(
            parse_inventory(
                &format!("ROOT|{r}\nfind: Permission denied"),
                "a",
                "adb",
                filter()
            )
            .is_err()
        );
        assert!(valid_path(r, &format!("{r}/file with ' quote.jpg")));
        assert_eq!(quote("a'b"), "'a'\"'\"'b'");
    }
    #[test]
    fn deletion_rejects_new_files_and_database() {
        let r = "/storage/emulated/0/Android/data/com.zing.zalo";
        let p =
            parse_inventory(&format!("ROOT|{r}\n1|20|{r}/old.mp4"), "a", "adb", filter()).unwrap();
        assert!(delete_script(&p, &p.files).unwrap().contains("readlink -f"));
        let mut f = p.files[0].clone();
        f.modified = cutoff(&p.filter.date).unwrap();
        assert!(delete_script(&p, &[f]).is_err());
        let mut f = p.files[0].clone();
        f.path = format!("{r}/messages.db");
        assert!(delete_script(&p, &[f]).is_err());
    }

    #[test]
    fn cancellation_does_not_start_adb() {
        let root = "/storage/emulated/0/Android/data/com.zing.zalo";
        let p = parse_inventory(
            &format!("ROOT|{root}\n1|20|{root}/old.mp4"),
            "phone",
            "nonexistent-adb",
            filter(),
        )
        .unwrap();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let result = delete(p, tx, Arc::new(AtomicBool::new(true))).unwrap();
        assert!(result.contains("Đã dừng"));
        assert!(result.contains("xóa 0 tệp"));
    }

    #[test]
    #[ignore = "Read-only scan of connected phone; requires ZALO_TEST_ADB and ZALO_TEST_SERIAL"]
    fn device_scan_read_only() {
        let adb = std::env::var("ZALO_TEST_ADB").unwrap();
        let serial = std::env::var("ZALO_TEST_SERIAL").unwrap();
        let p = scan(&adb, &serial, Filter::default()).unwrap();
        println!(
            "Scanned media: {}; old media: {}; old bytes: {}",
            p.scanned,
            p.files.len(),
            p.files.iter().map(|f| f.bytes).sum::<u64>()
        );
    }

    #[test]
    #[ignore = "Requires ZALO_TEST_ADB and ZALO_TEST_SERIAL; uses isolated temporary files only"]
    fn device_deletion_guards() {
        let adb = std::env::var("ZALO_TEST_ADB").unwrap();
        let serial = std::env::var("ZALO_TEST_SERIAL").unwrap();
        let root = report(
            &adb,
            &serial,
            "mktemp -d /data/local/tmp/dashboard-zalo-test.XXXXXX".into(),
            "test-setup",
        )
        .unwrap()
        .trim()
        .to_string();
        assert!(root.starts_with("/data/local/tmp/dashboard-zalo-test."));
        let setup = format!(
            "cd {} || exit 1; printf old > old.jpg; printf old > changed.mp4; printf old > target.jpg; ln -s target.jpg link.jpg; touch -t 202001010000 old.jpg changed.mp4 target.jpg; stat -c '%Y|%s|%n' old.jpg",
            quote(&root)
        );
        let result = report(&adb, &serial, setup, "test-setup").unwrap();
        let modified: i64 = result.split('|').next().unwrap().parse().unwrap();
        let files: Vec<_> = ["old.jpg", "changed.mp4", "link.jpg"]
            .iter()
            .map(|name| Media {
                path: format!("{root}/{name}"),
                modified,
                bytes: 3,
            })
            .collect();
        let p = Preview {
            serial: serial.clone(),
            adb: adb.clone(),
            filter: filter(),
            root: root.clone(),
            files,
            scanned: 3,
        };
        report(
            &adb,
            &serial,
            format!("printf altered > {}/changed.mp4", quote(&root)),
            "test-change",
        )
        .unwrap();
        // Only this isolated test substitutes the production root, never a user's Zalo directory.
        let script = delete_script(&p, &p.files)
            .unwrap()
            .replace(&quote(ROOT), &quote(&root));
        let result = report(&adb, &serial, script, "test-delete").unwrap();
        let verify = report(&adb, &serial, format!("cd {} || exit 1; [ ! -e old.jpg ] && [ -f changed.mp4 ] && [ -L link.jpg ] && [ -f target.jpg ] && echo VERIFIED; rm -f -- old.jpg changed.mp4 link.jpg target.jpg; cd /; rmdir {}", quote(&root), quote(&root)), "test-verify").unwrap();
        assert_eq!(result.lines().filter(|s| *s == "DELETED").count(), 1);
        assert_eq!(result.lines().filter(|s| *s == "SKIPPED").count(), 2);
        assert!(verify.contains("VERIFIED"));
    }
}
