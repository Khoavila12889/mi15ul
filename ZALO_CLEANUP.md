# Dọn ảnh và video Zalo theo ngày

Mở **Advanced Tools → Dọn dữ liệu Zalo theo ngày** sau khi chọn điện thoại.

1. Chọn ngày bắt đầu giữ lại, dạng `YYYY-MM-DD`, hoặc **Giữ 6 tháng gần nhất**.
2. Chọn **Ảnh**, **Video**, hoặc cả hai.
3. Bấm **Quét và xem trước** để xem số tệp, dung lượng và danh sách theo ngày.
4. Kiểm tra điện thoại, ngày, danh sách; đánh dấu xác nhận rồi bấm **Xóa tệp đã xem trước**.

Trong khi xóa, có tiến độ và nút **Dừng sau đợt hiện tại**. Dừng không khôi phục tệp đã xóa; hãy quét lại trước khi tiếp tục. Hỗ trợ cả ảnh JXL mà Zalo sử dụng, bên cạnh JPG, PNG, WEBP, HEIC/HEIF, GIF, AVIF và BMP.

Ngày được tính theo thời gian sửa đổi tệp, múi giờ UTC+7, không phải ngày gửi tin nhắn hoặc ngày chụp EXIF. Tệp đúng ngày mốc được giữ. Mặc định mốc là sáu tháng lịch trước ngày hiện tại.

Chỉ xử lý tệp thường có đuôi ảnh/video được nhận diện trong `/sdcard/Android/data/com.zing.zalo` của hồ sơ Android mà ADB truy cập. Không xóa cơ sở dữ liệu tin nhắn, không chạy `pm clear`, không xóa thư mục hoặc tài liệu. Các bản Zalo nhân bản, hồ sơ công việc, đường dẫn khác và tệp không có đuôi nhận diện không nằm trong phạm vi.

Ảnh/video đã xóa có thể không tải lại được khi mở tin nhắn. Không có thùng rác. Lịch sử tin nhắn vẫn được giữ, nhưng nội dung media đã xóa không còn trên máy.

Mỗi lần quét gắn với điện thoại và bộ lọc. Đổi điện thoại, ADB hoặc ngày/loại tệp sẽ vô hiệu hóa bản xem trước. Khi xóa, kiểm tra lại đường dẫn thật, ngày sửa đổi và kích thước; tệp đã đổi hoặc là liên kết sẽ bị bỏ qua. Khi mất kết nối hoặc lỗi một phần, xem số tệp đã xử lý rồi quét lại. Mỗi lệnh có giới hạn 10 phút. Đóng cửa sổ không hủy tác vụ đang chạy; kết quả vẫn gắn với thiết bị ban đầu.

Không yêu cầu root. Nếu Android chặn đọc thư mục, quá trình sẽ báo lỗi thay vì coi dữ liệu bị chặn là danh sách rỗng. Báo cáo quét và kết quả từng đợt xóa được lưu trên máy tính trong thư mục dữ liệu người dùng `android-dashboard/reports`. Báo cáo chứa tên tệp; không gửi lên mạng.

## Kiểm chứng

- Đã chạy kiểm thử toàn workspace và biên dịch ứng dụng Windows.
- Kiểm thử trên điện thoại bằng tệp tạm riêng xác nhận: xóa tệp cũ, bỏ qua tệp thay đổi và liên kết tượng trưng.
- Quét chỉ đọc trên V2206 với 244.017 ảnh/video thành công sau khi giới hạn 100 đường dẫn mỗi lượt stat. Không xóa dữ liệu Zalo thật khi kiểm thử module.
- Kiểm tra định dạng và Clippy nghiêm ngặt toàn workspace còn báo các vấn đề có sẵn ở phần khác; module mới không phát sinh cảnh báo Clippy.
