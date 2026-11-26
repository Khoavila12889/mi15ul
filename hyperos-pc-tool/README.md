# HyperOS PC Optimizer (Bank & VNeID Safe)

Công cụ chạy hoàn toàn trên **Windows PC qua ADB** để tối ưu máy Xiaomi / HyperOS nội địa Trung Quốc (đã về ROM gốc, khóa Bootloader), đảm bảo **không cài đặt APK lạ, không chạy dịch vụ ngầm hay tiến trình `app_process` trên điện thoại**.

## Cấu trúc thư mục
- `main.py`, `gui.py`, `cli.py`: Giao diện đồ họa (`tkinter`) và dòng lệnh (`--cli`).
- `adb_client.py`: Tự tìm `adb.exe` chuẩn trên Windows và giao tiếp với điện thoại.
- `optimizer.py`, `presets.py`: Fix thông báo (`MILLET_NO_RESTRICT_APP` + Doze + AppOps), đổi `system_locales`, Safe Debloat User 0, khóa bảo mật.
- `mtz_packager.py`: Chuyển đổi `.mtz` sang cấu trúc `.mrm` / `.mrc` của Xiaomi Themes trên PC, đẩy qua ADB, kiểm tra SHA-1 và mở màn hình **Áp dụng** thủ công.
- `themes/`: Chứa 5 theme Việt hóa trích xuất từ `HyperOS_Tool.apk` kèm `metadata_template.json`.
- `backups/`: Tự động sao lưu `system_locales`, `MILLET_NO_RESTRICT_APP` và danh sách gói đã gỡ theo serial máy.

## Cách chạy nhanh
1. Trên điện thoại: vào **Cài đặt → Giới thiệu điện thoại** chạm nhiều lần vào *Phiên bản OS* để mở **Tùy chọn nhà phát triển**, sau đó vào **Cài đặt bổ sung → Tùy chọn nhà phát triển** bật **Gỡ lỗi USB (USB debugging)**.
   *(Nếu thao tác nạp Theme hoặc ghi `settings` báo từ chối quyền trên một số máy Xiaomi, bật thêm **Gỡ lỗi USB (Cài đặt bảo mật / USB debugging - Security settings)** trong lúc cài rồi tắt lại ở bước cuối).*
2. Cắm cáp USB nối điện thoại với PC và bấm **Cho phép (Allow)** trên hộp thoại xác nhận RSA.
3. Nháy đúp `Run_HyperOS_PC_Tool.bat` (hoặc chạy `python main.py`).
4. Sau khi tối ưu xong, vào tab **4. Khóa Bảo mật (Bank-Ready)** bấm **Hoàn tất & Khóa bảo mật** để tắt **Tùy chọn nhà phát triển** và **Gỡ lỗi USB**, rồi rút cáp để sử dụng VNeID và các app Ngân hàng.

## Dùng qua dòng lệnh (CLI)
```powershell
python main.py --cli --help
python main.py --cli info
python main.py --cli fix-noti
python main.py --cli locale vi
python main.py --cli theme MTZOS4.mtz
python main.py --cli debloat scan
python main.py --cli bank-lock
```
