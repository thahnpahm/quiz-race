# Quiz Race – Midterm Showdown (bản Rust)

Game đố vui trên lớp kiểu Blooket. Máy chiếu chạy app này, học sinh vào bằng điện thoại.
9 nhóm (1–10, bỏ nhóm 5), 3 phần × 5 câu, 20 giây mỗi câu. Phần 1 đua xe, phần 2 bơi, phần 3 leo núi.
Viết lại từ bản Node.js `quiz-race.zip`: một file exe duy nhất, không cần Node, npm hay Internet.

## Cài và chạy
- Bấm đúp `QuizRace-2.0.0-win-x64-setup.exe` là xong: không có bước nào phải chọn, không cần quyền admin. Bộ cài tạo lối tắt Start menu và Desktop, cài xong tự mở game. Gỡ trong Settings → Apps.
- **Lần đầu mở**, Windows hỏi tường lửa: bấm **Allow** cho mạng Private, nếu không thì điện thoại không vào được.
- Học sinh quét QR trên màn hình (link `http://<IP máy>:3000`). Máy chiếu và điện thoại phải dùng chung Wi-Fi.
- Điện thoại quét mà không vào được (khác Wi-Fi, hoặc Wi-Fi trường/quán chặn các máy gọi nhau): bấm **Online link (Internet)** trong lobby. App tự chạy Cloudflare Tunnel (lần đầu tải `cloudflared.exe` khoảng 55 MB về `%LOCALAPPDATA%\Quiz Race`) và QR tự đổi sang link `https://….trycloudflare.com`, vào được từ bất kỳ mạng nào. Đóng app hoặc bấm **Use Wi-Fi link instead** là link tắt. Ai có link đều mở được trang người chơi trong lúc link còn chạy.

## Điều khiển (host)
Space hoặc Enter là nút chính (Start → End question → Next…). P (hoặc nút **Pause music** góc trên bên phải) tạm dừng/phát nhạc, hiệu ứng âm thanh vẫn kêu. M tắt/bật toàn bộ âm thanh. F11 bật/tắt toàn màn hình.
Hết mỗi phần, nhạc tự nhỏ dần rồi tắt trong lúc xem kết quả và thuyết trình; bấm Start phần sau là nhạc bật lại.
Nút **Auto-advance** bật/tắt tự chuyển câu. Bấm ✕ trên ô nhóm hai lần để xoá nhóm. Bấm **Reset game** hai lần để xoá điểm (các nhóm vẫn ở lại).

## Nhạc riêng
Trong lobby bấm **Music…** rồi chọn một file nhạc trên máy (mp3, m4a, aac, ogg, flac, wav). App chép file vào `%APPDATA%\Quiz Race\music.<đuôi>` và phát suốt ván, nhỏ hơn một chút lúc học sinh làm bài; bài dài hơn 4 phút thì chỉ giữ 4 phút đầu. Bấm **Default music** để quay về nhạc có sẵn. Nếu đổi loa/tai nghe khi game đang mở, hãy mở lại game để tiếng ra đúng thiết bị. File nhạc chỉ nằm trên máy bạn, không nằm trong bộ cài hay mã nguồn.

## Đổi câu hỏi
Trong lobby bấm **Questions file…**: app tạo `%APPDATA%\Quiz Race\questions.json` (bản sao bộ câu hỏi gốc) và mở thư mục chứa nó.
Sửa file xong thì bấm **Reset game** để nạp lại. Có thể đổi `title`, `question_seconds`, `teams` (tối đa 12 nhóm), số phần và số câu; mỗi câu có 2–4 đáp án, `answer` là số thứ tự của đáp án đúng, tính từ 0.
Nếu file bị lỗi, app báo lỗi ở lobby và dùng bộ câu hỏi có sẵn.
Cũng có thể mở một file khác: `QuizRace.exe đường\dẫn\quiz.json`.

## So với bản gốc
- Sửa lỗi QR: QR luôn chứa IP mạng LAN. Bản gốc ra `localhost` nếu mở host theo đúng hướng dẫn README.
- Sửa lỗi chèn HTML: server chỉ nhận avatar có trong danh sách. Bản gốc cho phép người chơi gửi `<style>` làm vỡ màn hình mọi máy.
- Host chạy ngay trên máy nên không còn mật khẩu mặc định "host" gửi qua mạng.
- Bỏ các giá trị viết cứng "/15", "/9 teams", "3 phần": số câu, số phần, số nhóm lấy theo file câu hỏi.
- Nhóm mất kết nối giữa câu không còn làm câu đó phải chờ hết giờ. Không thể rời nhóm giữa chừng làm mất điểm.
- Thêm nhạc nền vui nhộn (hai bản: lobby và lúc làm bài), hiệu ứng âm thanh, chuyển cảnh trượt mờ, điểm bay lên trên làn đua, pháo giấy ở màn kết quả. Toàn bộ âm thanh được tổng hợp trong code, không dùng file nào.
- Màu nền và thẻ là màu đặc, không gradient hay trong suốt (cả màn host lẫn trang điện thoại).
- Nút Online link: thay cho việc tự chạy cloudflared rồi mở `/host` bằng link công khai như bản gốc.
- Font Be Vietnam Pro nhúng sẵn, hiện đủ dấu tiếng Việt nếu bạn viết câu hỏi bằng tiếng Việt.

## Build
```
cargo test
cargo test -- --ignored online   # thử Online link thật (cần Internet)
cargo build --release
iscc /DAppVersion=2.0.0 installer\QuizRace.iss
```
`tools/fetch_assets.py` tải lại font, emoji và tạo icon.

## Mã nguồn
- `src/game.rs`: luật chơi và tính điểm (có test).
- `src/server.rs`: server LAN (trang điện thoại, long-poll trạng thái, API join/answer). Long-poll thay cho SSE vì tunnel Cloudflare giữ lại luồng SSE.
- `src/main.rs`: màn hình host (macroquad).
- `src/audio.rs`: nhạc và hiệu ứng âm thanh.
- `src/player.html`: trang trên điện thoại.

## Ghi công
- Emoji: [Noto Emoji](https://github.com/googlefonts/noto-emoji), Apache License 2.0 (`assets/emoji/LICENSE-NotoEmoji.txt`).
- Font: [Be Vietnam Pro](https://github.com/bettergui/BeVietnamPro), SIL Open Font License (`assets/OFL-BeVietnamPro.txt`).
- Câu hỏi: từ bản gốc (Midterm Review – Organizational Psychology in Business).
