"""Builds every app and site icon from the brand mark.
  pip install pillow resvg-py
  python tools/build-icons.py

The source is assets/brand/feather-black.svg (the designer's mark, traced),
plus assets/brand/og-base.png for the social card. The colours and the tile
shape are the ones in README.md, under "Brand". This writes the white mark and
the tile SVGs next to the black one, the app icons in assets/ and the site
icons and social card in site/. Run it from the repository root after changing
the mark or the brand colours, and commit what it writes.
"""

import io
import re
import struct
from pathlib import Path

import resvg_py
from PIL import Image

# Golden-hour blush: cream warming to a soft rose-peach, from the feather's
# quill (bottom left) to its tip (top right).
TILE_STOPS = ((0, '#FAF5EF'), (0.5, '#F6E2D6'), (1, '#EFC3AE'))
MARK_COLOUR = '#35261F'  # deep warm brown
# A thin warm edge, so the pale tile keeps its outline on a white taskbar.
EDGE, EDGE_OPACITY = '#5E3A2A', 0.24
SHADOW = '#3A2414'  # the macOS tile's drop shadow

MARK = Path('assets/brand/feather-black.svg').read_text()
VIEW_W, VIEW_H = (float(v) for v in re.search(r'viewBox="0 0 ([\d.]+) ([\d.]+)"', MARK).groups())
PATH = re.search(r' d="([^"]+)"', MARK).group(1)

# Most of the feather's weight is in the vane on the right; centring its box
# alone leaves it looking shoved right, so it moves left by this much of its
# own height.
OPTICAL_SHIFT = -0.057


def mark(cx, cy, height, colour):
    """The mark, `height` tall, centred (optically) on cx, cy."""
    scale = height / VIEW_H
    x = cx - VIEW_W * scale / 2 + OPTICAL_SHIFT * height
    y = cy - height / 2
    return f'<path transform="translate({x:.2f} {y:.2f}) scale({scale:.5f})" fill="{colour}" d="{PATH}"/>'


def tile_svg(size=1024, inset=0.03125, radius=0.225, mark_height=0.64, edge=0.012, shadow=False):
    """The blush tile with the mark. All lengths are fractions of `size`."""
    side = size * (1 - 2 * inset)
    at = size * inset
    rx = side * radius
    stops = ''.join(f'<stop offset="{o:g}" stop-color="{c}"/>' for o, c in TILE_STOPS)
    defs = (
        f'<linearGradient id="g" gradientUnits="userSpaceOnUse" x1="{at + side * 0.15:g}" y1="{at + side * 0.95:g}" '
        f'x2="{at + side * 0.9:g}" y2="{at + side * 0.05:g}">{stops}</linearGradient>'
    )
    filt = ''
    if shadow:
        defs += (
            '<filter id="s" x="-20%" y="-20%" width="140%" height="140%">'
            f'<feDropShadow dx="0" dy="{size * 0.012:.1f}" stdDeviation="{size * 0.014:.1f}" '
            f'flood-color="{SHADOW}" flood-opacity=".28"/></filter>'
        )
        filt = ' filter="url(#s)"'
    outline = ''
    if edge:
        w = size * edge
        outline = (
            f'  <rect x="{at + w / 2:g}" y="{at + w / 2:g}" width="{side - w:g}" height="{side - w:g}" '
            f'rx="{max(rx - w / 2, 0):.1f}" fill="none" stroke="{EDGE}" stroke-opacity="{EDGE_OPACITY}" '
            f'stroke-width="{w:g}"/>\n'
        )
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}">\n'
        f'  <title>Feather</title>\n'
        f'  <defs>{defs}</defs>\n'
        f'  <rect x="{at:g}" y="{at:g}" width="{side:g}" height="{side:g}" rx="{rx:.1f}" fill="url(#g)"{filt}/>\n'
        f'{outline}'
        f'  {mark(size / 2, size / 2, size * mark_height, MARK_COLOUR)}\n'
        f'</svg>\n'
    )


def render(svg, px):
    png = resvg_py.svg_to_bytes(svg_string=svg, width=px, height=px)
    return Image.open(io.BytesIO(bytes(png))).convert('RGBA')


# The tile at three sizes. Small icons get the whole square and a bigger mark
# so the feather still reads in a taskbar or a browser tab.
# The edge is thicker at small sizes, where a hairline would disappear.
LARGE = dict()  # 64 px and up
MEDIUM = dict(inset=0, radius=0.22, mark_height=0.72, edge=0.028)  # 25 to 48 px
SMALL = dict(inset=0, radius=0.2, mark_height=0.8, edge=0.05)  # 24 px and under


def app_icon(px):
    tier = SMALL if px <= 24 else MEDIUM if px <= 48 else LARGE
    return render(tile_svg(**tier), px)


def and_mask(im):
    """The 1-bit mask under a bitmap icon: set where the pixel is fully
    transparent, for anything that draws icons by mask rather than by alpha.
    Bottom-up rows, most significant bit first, each padded to 4 bytes."""
    w, h = im.size
    alpha = im.getchannel('A').tobytes()
    stride = ((w + 31) // 32) * 4
    out = bytearray()
    for y in range(h - 1, -1, -1):
        row = bytearray(stride)
        for x in range(w):
            if alpha[y * w + x] == 0:
                row[x // 8] |= 0x80 >> (x % 8)
        out += row
    return bytes(out)


def write_ico(path, images):
    """An .ico with each size drawn for that size: classic 32-bit bitmaps up to
    128 px and PNG at 256, the layout Windows and resource compilers expect."""
    entries, blobs = [], []
    offset = 6 + 16 * len(images)
    for im in images:
        w, h = im.size
        if w >= 256:
            buf = io.BytesIO()
            im.save(buf, 'PNG', optimize=True)
            blob = buf.getvalue()
        else:
            header = struct.pack('<IiiHHIIiiII', 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
            rows = [im.crop((0, y, w, y + 1)).tobytes('raw', 'BGRA') for y in range(h - 1, -1, -1)]
            blob = header + b''.join(rows) + and_mask(im)
        entries.append(struct.pack('<BBBBHHII', w % 256, h % 256, 0, 0, 1, 32, len(blob), offset))
        blobs.append(blob)
        offset += len(blob)
    Path(path).write_bytes(struct.pack('<HHH', 0, 1, len(images)) + b''.join(entries) + b''.join(blobs))


def save_png(im, path):
    im.save(path, 'PNG', optimize=True)
    print('wrote', path)


def og_image():
    """The social card: assets/brand/og-base.png (a screenshot of the site with
    an empty spot, top left) with the tile pasted in. Built from the same two
    inputs every time, so a new mark or colour always reaches it and a rerun
    with nothing changed writes the same bytes."""
    card = Image.open('assets/brand/og-base.png').convert('RGB')
    tile = render(tile_svg(**MEDIUM), 54 * 4).resize((54, 54), Image.LANCZOS)
    card.paste(tile, (65, 53), tile)
    card.save('site/og.jpg', quality=90, optimize=True, progressive=True)
    print('wrote site/og.jpg')


def white_mark():
    """The one-colour white mark, from the black one, so the two never drift apart."""
    white = MARK.replace('fill="#000000"', 'fill="#FFFFFF"')
    assert white != MARK, 'feather-black.svg should fill its path with #000000'
    Path('assets/brand/feather-white.svg').write_text(white)
    print('wrote assets/brand/feather-white.svg')


def main():
    brand = Path('assets/brand')
    white_mark()
    # Windows, Linux, the window icon and the web.
    for name, tier in (('icon.svg', LARGE), ('icon-48.svg', MEDIUM), ('icon-24.svg', SMALL)):
        (brand / name).write_text(tile_svg(**tier))
        print('wrote', brand / name)
    # macOS: Apple's grid, an 824 px tile on a 1024 px canvas with a soft shadow.
    macos = tile_svg(inset=100 / 1024, radius=0.225, mark_height=0.52, shadow=True)
    (brand / 'icon-macos.svg').write_text(macos)
    print('wrote', brand / 'icon-macos.svg')

    save_png(app_icon(256), 'assets/icon.png')
    save_png(render(macos, 1024), 'assets/icon-1024.png')
    write_ico('assets/icon.ico', [app_icon(px) for px in (16, 20, 24, 32, 40, 48, 64, 128, 256)])
    print('wrote assets/icon.ico')

    # Browsers that take an SVG favicon get the small drawing, sharp at any size.
    Path('site/favicon.svg').write_text(tile_svg(**SMALL))
    print('wrote site/favicon.svg')
    # The logo in the site's header, shown at 26 px.
    Path('site/logo.svg').write_text(tile_svg(**MEDIUM))
    print('wrote site/logo.svg')
    save_png(app_icon(64), 'site/favicon.png')
    save_png(app_icon(512), 'site/icon.png')
    # iOS rounds the corners itself and wants no transparency.
    save_png(render(tile_svg(inset=0, radius=0, mark_height=0.62, edge=0), 180).convert('RGB'), 'site/apple-touch-icon.png')
    og_image()


if __name__ == '__main__':
    main()
