"""Tải font Be Vietnam Pro và emoji Noto, tạo icon.ico và src/emoji_list.rs.
Chạy: python -I tools/fetch_assets.py   (trong thư mục quiz-race)"""
import io, sys, urllib.request
from pathlib import Path
from PIL import Image

ROOT = Path(sys.argv[0]).resolve().parent.parent
A = ROOT / "assets"
NOTO = "https://raw.githubusercontent.com/googlefonts/noto-emoji/main/"
FONTS = "https://raw.githubusercontent.com/google/fonts/main/ofl/bevietnampro/"

AVATARS = "🦊 🐼 🐯 🦁 🐸 🐵 🐙 🦄 🐧 🐨 🐰 🐻 🦉 🐢 🦖 🐬 🦈 🐝 🦋 🐞 👽 🤖 👻 🎃 🍕 🍔 🚀 ⚡ 🔥 🌈".split()
ICONS = "🏁 🎉 🏎️ 🏊 🥾 🏔️ 🏅 💦 🎒 🥇 🥈 🥉 🏆 🎖️ ✅ ❌ ✋ ⏰".split()

def get(url):
    with urllib.request.urlopen(url, timeout=30) as r:
        return r.read()

def code(e):
    return "_".join(f"{ord(c):x}" for c in e if ord(c) != 0xFE0F)

for name in ["BeVietnamPro-SemiBold.ttf", "BeVietnamPro-Bold.ttf", "OFL.txt"]:
    p = A / ("OFL-BeVietnamPro.txt" if name == "OFL.txt" else name)
    if not p.exists():
        p.write_bytes(get(FONTS + name))

(A / "emoji" / "LICENSE-NotoEmoji.txt").write_bytes(get(NOTO + "LICENSE"))
lines = ["// Sinh bởi tools/fetch_assets.py – emoji Noto (Apache 2.0)", "pub const EMOJI: &[(&str, &[u8])] = &["]
for e in AVATARS + ICONS:
    c = code(e)
    p = A / "emoji" / f"{c}.png"
    if not p.exists():
        p.write_bytes(get(f"{NOTO}2D/png/128/emoji_u{c}.png"))
    lines.append(f'    ("{e}", include_bytes!("../assets/emoji/{c}.png")),')
lines.append("];")
lines.append("pub const AVATARS: &[&str] = &[" + ", ".join(f'"{e}"' for e in AVATARS) + "];")
(ROOT / "src" / "emoji_list.rs").write_text("\n".join(lines) + "\n", encoding="utf-8")

# icon: cờ đua trên nền tròn màu tím của game
flag = Image.open(io.BytesIO(get(NOTO + "2D/png/512/emoji_u1f3c1.png"))).convert("RGBA")
from PIL import ImageDraw
base = Image.new("RGBA", (512, 512), (0, 0, 0, 0))
ImageDraw.Draw(base).rounded_rectangle((8, 8, 504, 504), 110, fill=(91, 36, 122, 255))
f = flag.resize((400, 400), Image.LANCZOS)
base.alpha_composite(f, (56, 50))
base.save(A / "icon-512.png")
base.save(ROOT / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
for s in (16, 32, 64):
    base.resize((s, s), Image.LANCZOS).save(A / f"icon-{s}.png")
print("ok")
