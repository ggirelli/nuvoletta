# Nuvoletta cloud-chat identity

The symbol merges a soft cloud silhouette with a comic speech bubble. Three rounded strokes inside suggest lines of writing or a thought being composed. The speech tail is integrated into the cloud contour, so the mark reads as a little cloud that can hold thoughts—not as a generic messaging app or cloud-storage service.

## Palette
- Sky Blue: `#4C7898` — primary mark
- Deep Fjord: `#294D68` — wordmark and dark accent
- Cloud White: `#F2F7FA` — light text and surfaces
- Mist: `#E8F1F6` — pale background
- Night: `#121212` — dark UI background

Contrast:
- Sky Blue on white: 4.72:1
- Sky Blue on #121212: 3.97:1
- Deep Fjord on white: 8.91:1
- Cloud White on #121212: 17.36:1

Use Deep Fjord for small text on light backgrounds and Cloud White for small text on dark backgrounds. Sky Blue is suitable for graphical elements on both.

## Clear space and sizing
Keep clear space around the mark equal to at least 25% of its width.
- Logo mark: 24 px minimum (use simplified favicon below 24 px).
- Wordmark lockup: 180 px minimum width.
- Favicon: use the dedicated 32 px or 16 px file.

## Do
- Keep the mark flat, rounded, and unembellished.
- Preserve proportions and internal line spacing.
- Use the supplied palette and generous whitespace.
- Use the monochrome mark for one-colour contexts.

## Don't
- Do not add locks, shields, keys, upload arrows, binary patterns, or encryption clichés.
- Do not add eyes or a face; the symbol should remain a calm writing object, not a mascot.
- Do not stretch, rotate, outline, shadow, bevel, or decorate the mark.
- Do not use it as a literal weather or cloud-sync icon.

## Typography
Wordmarks and banners use live text with a system sans-serif stack: `system-ui`, `-apple-system`, `BlinkMacSystemFont`, `Segoe UI`, sans-serif. No external fonts are loaded and text is not converted to paths.

## Assets
All assets are original SVG source files. Release under the MIT License.

## Regenerating
All SVGs are generated from the single mark definition in `generate.py`: icons are centred and
scaled to fill their canvas with a small padding; favicons are the app icon in miniature (white mark
on the blue tile, with heavier strokes at small sizes); banners have three overlapping gradient
cloud banks that span the bottom and rise at both sides.
Edit the mark, palette or layout there and run `python3 assets/brand/generate.py`; do not edit the
SVGs by hand.
