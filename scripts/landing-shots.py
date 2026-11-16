"""Les images du site (`landing/`) à partir des captures de l'application.

Les captures viennent du test `captures_du_site`, en double densité (`target/captures/site-*.png`),
en anglais pour le site et en français pour le README. Ce script en tire les trois vues du
cadre, l'image de partage `og.jpg`, l'icône, et l'image d'en-tête du README.

Usage : cargo test captures_du_site -- --ignored && python scripts/landing-shots.py   (demande Pillow)
"""
import os

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

BG, GLOW, LINE = (17, 17, 16), (244, 169, 69), (244, 169, 69)


def stage(capture, size, width, top):
    """La fenêtre posée sur le fond du site, sous son halo, avec une ombre et un filet clair."""
    W, H = size
    out = Image.new("RGB", size, BG)
    halo = Image.new("L", size, 0)
    ImageDraw.Draw(halo).ellipse((W * 0.12, -H * 0.62, W * 0.88, H * 0.5), fill=70)
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
    ImageDraw.Draw(border).rounded_rectangle((box[0] - 2, box[1] - 2, box[2] + 2, box[3] + 2), radius + 2, fill=44)
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

# L'icône du site : celle de l'application, son triangle de lecture passé à l'ambre du site.
LIME, AMBER = ("#d4f76c", "#a9d62e"), ("#ffbb62", "#ee9f38")
svg = open(os.path.join(ROOT, "assets", "icon.svg"), encoding="utf-8").read()
for lime, amber in zip(LIME, AMBER):
    assert lime in svg, f"couleur {lime} absente de l'icône"
    svg = svg.replace(lime, amber)
open(os.path.join(LANDING, "icon.svg"), "w", encoding="utf-8", newline="\n").write(svg)

icon = Image.open(os.path.join(ROOT, "assets", "icon.png")).convert("RGBA")
top, bottom = [tuple(int(c[i:i + 2], 16) for i in (1, 3, 5)) for c in AMBER]
pixels = icon.load()
for y in range(icon.height):
    k = y / max(icon.height - 1, 1)
    for x in range(icon.width):
        r, g, b, a = pixels[x, y]
        # Le triangle est le seul vert de l'icône : il prend le dégradé ambre, à la même force.
        if g - b > 50 and g > r:
            force = min((g - b) / 150, 1.0)
            pixels[x, y] = tuple(round((top[i] * (1 - k) + bottom[i] * k) * force + (r, g, b)[i] * (1 - force)) for i in range(3)) + (a,)
icon.resize((128, 128), Image.LANCZOS).save(os.path.join(LANDING, "icon", "128.png"), optimize=True)
print("ok")
