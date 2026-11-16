"""Les images du site (`landing/`) à partir des captures de l'application.

Les captures viennent du test `captures_du_site`, en double densité (`target/captures/site-*.png`),
en anglais pour le site et en français pour le README. Ce script en tire les trois vues du
cadre, l'image de partage `og.jpg`, l'icône, et l'image d'en-tête du README.

Usage : cargo test captures_du_site -- --ignored && python scripts/landing-shots.py   (demande Pillow)
"""
import os
import shutil

from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CAPTURES = os.path.join(ROOT, "target", "captures")
LANDING = os.path.join(ROOT, "landing")
VIEWS = ["choix", "reglages-fins", "en-cours"]

os.makedirs(os.path.join(LANDING, "shots"), exist_ok=True)
os.makedirs(os.path.join(LANDING, "icon"), exist_ok=True)

for view in VIEWS:
    shot = Image.open(os.path.join(CAPTURES, f"site-en-{view}.png")).convert("RGB")
    shot.save(os.path.join(LANDING, "shots", f"{view}.webp"), quality=88, method=6)

BG, GLOW, LINE = (12, 12, 12), (244, 169, 69), (255, 255, 255)


def stage(capture, size, width, top):
    """La fenêtre posée sur le fond du site, sous son halo, avec une ombre et un filet clair."""
    W, H = size
    out = Image.new("RGB", size, BG)
    halo = Image.new("L", size, 0)
    ImageDraw.Draw(halo).ellipse((W * 0.12, -H * 0.62, W * 0.88, H * 0.5), fill=30)
    out = Image.composite(Image.new("RGB", size, GLOW), out, halo.filter(ImageFilter.GaussianBlur(160)))
    shot = Image.open(os.path.join(CAPTURES, capture)).convert("RGB")
    shot = shot.resize((width, round(shot.height * width / shot.width)), Image.LANCZOS)
    radius = 30
    x = (W - width) // 2
    box = (x, top, x + shot.width - 1, top + shot.height - 1)
    shadow = Image.new("L", size, 0)
    ImageDraw.Draw(shadow).rounded_rectangle((box[0], box[1] + 40, box[2], box[3] + 40), radius, fill=200)
    out = Image.composite(Image.new("RGB", size, (0, 0, 0)), out, shadow.filter(ImageFilter.GaussianBlur(60)))
    border = Image.new("L", size, 0)
    ImageDraw.Draw(border).rounded_rectangle((box[0] - 2, box[1] - 2, box[2] + 2, box[3] + 2), radius + 2, fill=34)
    out = Image.composite(Image.new("RGB", size, LINE), out, border)
    mask = Image.new("L", shot.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, shot.width - 1, shot.height - 1), radius, fill=255)
    out.paste(shot, (x, top), mask)
    return out


# L'image de partage du site : la fenêtre dépasse par le bas.
stage("site-en-choix.png", (2400, 1260), 1840, 150).save(os.path.join(LANDING, "og.jpg"), quality=88, optimize=True, progressive=True)
# L'en-tête du README : la fenêtre entière.
os.makedirs(os.path.join(ROOT, ".github", "assets"), exist_ok=True)
stage("site-fr-choix.png", (2400, 1560), 1840, 150).save(os.path.join(ROOT, ".github", "assets", "hero.jpg"), quality=88, optimize=True, progressive=True)

shutil.copyfile(os.path.join(ROOT, "assets", "icon.svg"), os.path.join(LANDING, "icon.svg"))
Image.open(os.path.join(ROOT, "assets", "icon.png")).resize((128, 128), Image.LANCZOS).save(os.path.join(LANDING, "icon", "128.png"), optimize=True)
print("ok")
