#!/usr/bin/env python3
"""Regenerate the Nuvoletta brand SVGs from the single mark definition.

Run from anywhere: `python3 assets/brand/generate.py`. Every asset is derived from MARK below, so
changing the mark (or the palette) here and re-running keeps all files consistent.
"""

from pathlib import Path

OUT = Path(__file__).resolve().parent

# Palette (see README.md).
SKY = "#4C7898"
FJORD = "#294D68"
CLOUD_WHITE = "#F2F7FA"
MIST = "#E8F1F6"
ICE = "#B9D7E8"

# The mark, drawn in a 1024 box. Its visual bounds (strokes included) are measured once with
# `inkscape --query-all`: x 130..894, y 130..742.28.
MARK_PATHS = """<path d="M512 154 C414 154 342 208 320 286 C225 279 154 346 154 435 C154 515 210 579 287 592 L258 704 Q253 724 273 716 L391 667 C426 681 465 690 507 690 C604 690 681 638 704 557 C797 548 870 479 870 390 C870 298 795 230 700 233 C666 185 596 154 512 154 Z" fill="none" stroke="currentColor" stroke-width="48" stroke-linecap="round" stroke-linejoin="round"/>
<path d="M365 374 L648 338 M378 462 L662 426 M405 550 L580 528" fill="none" stroke="currentColor" stroke-width="43" stroke-linecap="round" stroke-linejoin="round"/>"""
MARK_BOX = (130.0, 130.0, 894.0, 742.28)

FONT = "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"


def fit(box, cx, cy, size):
    """Transform that centres `box` on (cx, cy) with its longer side equal to `size`."""
    x0, y0, x1, y1 = box
    s = size / max(x1 - x0, y1 - y0)
    tx = cx - s * (x0 + x1) / 2
    ty = cy - s * (y0 + y1) / 2
    return f"translate({tx:.3f} {ty:.3f}) scale({s:.5f})", s


def mark(color, cx, cy, size):
    t, _ = fit(MARK_BOX, cx, cy, size)
    return f'<g transform="{t}" style="color:{color}">\n{MARK_PATHS}\n</g>'


def svg(w, h, body, vb=None):
    vb = vb or f"0 0 {w} {h}"
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{vb}" '
        f'role="img">\n{body}\n</svg>\n'
    )


def write(name, content):
    (OUT / name).write_text(content)


# --- Icons ------------------------------------------------------------------------------------

# Plain mark and monochrome: as large as possible with a small (4%) padding.
write("01-logo-mark.svg", svg(1024, 1024, mark(SKY, 512, 512, 1024 - 2 * 40)))
write("05-monochrome.svg", svg(1024, 1024, mark("#000000", 512, 512, 1024 - 2 * 40)))

# App icon: the rounded tile keeps its corners clear, so the mark gets a little more padding.
write(
    "02-app-icon.svg",
    svg(
        1024,
        1024,
        '<defs><linearGradient id="tile" x1="0" y1="0" x2="1" y2="1">'
        f'<stop offset="0" stop-color="{SKY}"/><stop offset="1" stop-color="{FJORD}"/>'
        "</linearGradient></defs>\n"
        '<rect width="1024" height="1024" rx="220" fill="url(#tile)"/>\n'
        + mark(CLOUD_WHITE, 512, 512, 1024 - 2 * 128),
    ),
)

# Android adaptive icon: launchers mask the 108 dp layer down to as little as a 66 dp circle, so
# the mark stays inside that circle.
write("03-android-foreground.svg", svg(108, 108, mark(FJORD, 54, 54, 58)))
write("04-android-background.svg", svg(108, 108, f'<rect width="108" height="108" fill="{MIST}"/>'))

# Favicons: the app icon in miniature (white mark on a blue rounded tile), which stays legible on
# both light and dark browser chrome. At 16 px the standard strokes would be under 1 px wide and
# three lines blur together, so that size uses heavier strokes and two lines.
def tile(size, radius, content):
    return (
        f'<defs><linearGradient id="tile" x1="0" y1="0" x2="1" y2="1">'
        f'<stop offset="0" stop-color="{SKY}"/><stop offset="1" stop-color="{FJORD}"/>'
        "</linearGradient></defs>\n"
        f'<rect width="{size}" height="{size}" rx="{radius}" fill="url(#tile)"/>\n{content}'
    )


MEDIUM = MARK_PATHS.replace('stroke-width="48"', 'stroke-width="66"').replace(
    'stroke-width="43"', 'stroke-width="60"'
)
MEDIUM_BOX = (121.0, 121.0, 903.0, 751.28)  # MARK_BOX grown by the extra stroke width
t, _ = fit(MEDIUM_BOX, 16, 16, 26)
write(
    "06a-favicon-32.svg",
    svg(32, 32, tile(32, 7, f'<g transform="{t}" style="color:{CLOUD_WHITE}">\n{MEDIUM}\n</g>')),
)
OUTLINE = MARK_PATHS.split("\n")[0].replace('stroke-width="48"', 'stroke-width="84"')
FAV16_PATHS = OUTLINE + (
    '\n<path d="M360 390 L660 352 M380 520 L640 487" fill="none" stroke="currentColor" '
    'stroke-width="84" stroke-linecap="round"/>'
)
FAV16_BOX = (112.0, 112.0, 912.0, 760.28)  # MARK_BOX grown by the extra stroke width
t, _ = fit(FAV16_BOX, 8, 8, 14)
write(
    "06b-favicon-16.svg",
    svg(16, 16, tile(16, 3.5, f'<g transform="{t}" style="color:{CLOUD_WHITE}">\n{FAV16_PATHS}\n</g>')),
)


# --- Wordmarks --------------------------------------------------------------------------------

def wordmark(mark_color, text_color):
    # Mark fills the height with a 10 px padding; text is vertically centred on it.
    h, pad = 150, 10
    size = h - 2 * pad
    _, s = fit(MARK_BOX, 0, 0, size)
    mark_w = s * (MARK_BOX[2] - MARK_BOX[0])
    mx = pad + mark_w / 2
    text_x = pad + mark_w + 22
    width = round(text_x + 352 + pad)
    body = mark(mark_color, mx, h / 2, size) + (
        f'\n<text x="{text_x:.1f}" y="{h / 2 + 22:.1f}" fill="{text_color}" font-family="{FONT}" '
        'font-size="74" font-weight="650" letter-spacing="-2.2">nuvoletta</text>'
    )
    return svg(width, h, body)


write("07a-wordmark-light.svg", wordmark(SKY, FJORD))
write("07b-wordmark-dark.svg", wordmark(ICE, CLOUD_WHITE))


# --- Banners ----------------------------------------------------------------------------------
import math


def bank(w, h, base, rise, radius, phase, spacing=1.05):
    """Bumps of a cloud bank spanning the whole width: low in the middle, rising at both sides.

    `base` and `rise` are fractions of the height for the bumps' tops in the middle and the extra
    height at the edges; `phase` varies the bump sizes so layers do not repeat each other.
    """
    bumps, x, i = [], -radius, 0
    while x < w + radius:
        r = radius * (1 + 0.28 * math.sin(i * 1.9 + phase) + 0.12 * math.sin(i * 0.7 + 2 * phase))
        edge = abs(x - w / 2) / (w / 2)
        top = h * (1 - base - rise * edge ** 2.4) + 0.08 * r * math.sin(i * 2.3 + phase)
        bumps.append((x, top + r, r))
        x += r * spacing
        i += 1
    return bumps


def bank_svg(bumps, h, fill, opacity):
    """Circles plus a polygon through their centres down to the bottom: one seamless shape."""
    circles = "".join(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{r:.1f}"/>' for x, y, r in bumps)
    pts = " ".join(f"{x:.1f},{y:.1f}" for x, y, _ in bumps)
    x0, x1 = bumps[0][0], bumps[-1][0]
    poly = f'<polygon points="{x0:.1f},{h + 10} {pts} {x1:.1f},{h + 10}"/>'
    return f'<g fill="{fill}" opacity="{opacity}">{circles}{poly}</g>'


def gradient(gid, y0, y1, top, bottom):
    return (
        f'<linearGradient id="{gid}" gradientUnits="userSpaceOnUse" x1="0" y1="{y0:.0f}" x2="0" '
        f'y2="{y1:.0f}"><stop offset="0" stop-color="{top}"/>'
        f'<stop offset="1" stop-color="{bottom}"/></linearGradient>'
    )


def banner(w, h, dark, mark_size, title_size, tag_size, cloud_r):
    if dark:
        sky = ("#0E2440", "#1B3F66")
        # (top, bottom) colours of each layer, back to front.
        layers = [("#2D5A86", "#1E4670"), ("#3A6C9A", "#24507C"), ("#4B7FAE", "#2E5E8C")]
        mark_c, title_c, tag_c = CLOUD_WHITE, CLOUD_WHITE, ICE
    else:
        sky = ("#F1F8FD", "#CFE6F7")
        layers = [("#DCEEFA", "#B9DAF2"), ("#EEF7FD", "#C8E3F6"), ("#FFFFFF", "#DCEEFA")]
        mark_c, title_c, tag_c = "#2F6496", FJORD, "#2F6496"

    # Back to front: each layer is lower in the middle, rises less at the edges, has smaller bumps
    # and a different phase, so the banks overlap with an offset instead of repeating.
    specs = [(0.30, 0.42, 1.25, 0.3, 0.85), (0.20, 0.34, 1.0, 1.7, 0.92), (0.10, 0.24, 0.8, 3.1, 1.0)]
    defs, banks = [], []
    for n, ((base, rise, rf, phase, op), (top, bottom)) in enumerate(zip(specs, layers)):
        y0 = h * (1 - base - rise)
        defs.append(gradient(f"c{n}", y0, h, top, bottom))
        banks.append(bank_svg(bank(w, h, base, rise, cloud_r * rf, phase), h, f"url(#c{n})", op))

    _, s = fit(MARK_BOX, 0, 0, mark_size)
    mark_w = s * (MARK_BOX[2] - MARK_BOX[0])
    gap = mark_size * 0.14
    # Text width estimate for the system sans-serif (about 0.53 em per glyph at weight 650).
    text_w = max(9 * 0.53 * title_size, 29 * 0.47 * tag_size)
    gx = (w - (mark_w + gap + text_w)) / 2
    cy = h * 0.40
    text_x = gx + mark_w + gap
    body = (
        f'<defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">'
        f'<stop offset="0" stop-color="{sky[0]}"/><stop offset="1" stop-color="{sky[1]}"/>'
        f"</linearGradient>{''.join(defs)}</defs>\n"
        f'<rect width="{w}" height="{h}" fill="url(#bg)"/>\n'
        + "\n".join(banks)
        + "\n"
        + mark(mark_c, gx + mark_w / 2, cy, mark_size)
        + f'\n<text x="{text_x:.1f}" y="{cy - 0.05 * title_size:.1f}" fill="{title_c}" '
        f'font-family="{FONT}" font-size="{title_size}" font-weight="650" '
        f'letter-spacing="{-0.035 * title_size:.1f}">nuvoletta</text>'
        f'\n<text x="{text_x + 0.04 * title_size:.1f}" y="{cy + 0.05 * title_size + tag_size:.1f}" '
        f'fill="{tag_c}" font-family="{FONT}" font-size="{tag_size}" font-weight="450">'
        "Private thoughts. Safely kept.</text>"
    )
    return svg(w, h, body)


write("08-github-social-preview.svg", banner(1280, 640, False, 250, 104, 38, 88))
write("09-github-social-preview-dark.svg", banner(1280, 640, True, 250, 104, 38, 88))
write("10-docs-hero-light.svg", banner(1600, 400, False, 190, 84, 31, 62))
write("11-docs-hero-dark.svg", banner(1600, 400, True, 190, 84, 31, 62))
