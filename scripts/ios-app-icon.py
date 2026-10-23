#!/usr/bin/env python3
"""Draws the iOS app icon from Spotifast's mark, or checks the committed one.

    python3 scripts/ios-app-icon.py           writes the icon
    python3 scripts/ios-app-icon.py --check   checks the committed icon

iOS shows an app icon as an opaque square under a rounded mask of its own.
The round mark in packaging/icons/spotifast.svg leaves its corners
transparent, and packaging/macos/icon-1024.png sets the disc in a transparent
margin with a drop shadow, so neither can be used as it is. The icon is the
SVG's disc at 85 % of the side, over a square painted with the disc's own rim
gradient: the rim merges into the background, and the face, its bevel and
the play mark remain. It is 1024 px with no alpha channel, since actool and
App Store validation reject transparency in an app icon; Xcode makes every
smaller size from it (a single-size app icon, Xcode 14 or newer).

The PNG is committed: run this again only when the SVG changes. It needs
Pillow (python3 -m pip install pillow) and librsvg, through rsvg-convert or
through ImageMagick 7 (magick) built with it.
"""

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
MARK = ROOT / "packaging" / "icons" / "spotifast.svg"
ICON = ROOT / "ios" / "Assets.xcassets" / "AppIcon.appiconset" / "AppIcon-1024.png"
SIDE = 1024
# The disc's diameter, as a share of the icon's side.
DISC = 0.85


def square(svg):
    """The mark in a square view box where its disc spans DISC of the side,
    over a background painted with the gradient that fills the disc's rim."""
    rim = re.search(r'<circle cx="([\d.]+)" cy="([\d.]+)" r="([\d.]+)" fill="url\(#([\w-]+)\)"\s*/>', svg)
    root = re.search(r"<svg\b[^>]*>", svg)
    if rim is None or root is None or "</defs>" not in svg:
        sys.exit(f"{MARK}: expected an <svg> with <defs> and a disc filled with a gradient")
    cx, cy, r = (float(rim.group(i)) for i in (1, 2, 3))
    side = 2 * r / DISC
    x, y = cx - side / 2, cy - side / 2
    opening = (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x:g} {y:g} {side:g} {side:g}" '
        f'width="{SIDE}" height="{SIDE}">'
    )
    # Well past the view box on every side, so no pixel at the edge is only
    # partly covered. The gradient is in user space and pads with its end
    # colours, so above and below the disc the background holds the rim's
    # top and bottom colour, and across the disc it matches the rim exactly.
    background = (
        f'<rect x="{x - side:g}" y="{y - side:g}" width="{3 * side:g}" height="{3 * side:g}" '
        f'fill="url(#{rim.group(4)})"/>'
    )
    return opening + svg[root.end() :].replace("</defs>", "</defs>" + background, 1)


def render(folder, svg, png):
    """Renders folder/svg to folder/png at SIDE px with librsvg."""
    if shutil.which("rsvg-convert"):
        command = ["rsvg-convert", "--width", str(SIDE), "--height", str(SIDE), "--output", png, svg]
    elif shutil.which("magick"):
        # RSVG: makes ImageMagick use librsvg rather than its own renderer, and
        # at 96 dpi one SVG pixel is one image pixel.
        command = ["magick", "-background", "none", "-density", "96", f"RSVG:{svg}", f"PNG32:{png}"]
    else:
        sys.exit("librsvg is needed: rsvg-convert, or ImageMagick 7 (magick) built with it")
    subprocess.run(command, cwd=folder, check=True)


def problems(image):
    """What keeps image from being an iOS app icon, if anything."""
    found = []
    if image.size != (SIDE, SIDE):
        found.append(f"is {image.width}x{image.height}, not {SIDE}x{SIDE}")
    if image.convert("RGBA").getchannel("A").getextrema() != (255, 255):
        found.append("has pixels that are not fully opaque")
    if "A" in image.getbands() or "transparency" in image.info:
        found.append(f"has an alpha channel or a transparent colour (mode {image.mode})")
    return found


def draw():
    with tempfile.TemporaryDirectory() as folder:
        Path(folder, "icon.svg").write_text(square(MARK.read_text(encoding="utf-8")), encoding="utf-8")
        render(folder, "icon.svg", "icon.png")
        with Image.open(Path(folder, "icon.png")) as rendered:
            rendered.load()
            if rendered.size != (SIDE, SIDE):
                sys.exit(f"the renderer made {rendered.width}x{rendered.height}, not {SIDE}x{SIDE}")
            if rendered.getchannel("A").getextrema() != (255, 255):
                sys.exit("the background does not cover the whole square")
            icon = rendered.convert("RGB")
    ICON.parent.mkdir(parents=True, exist_ok=True)
    icon.save(ICON, optimize=True)


def check():
    with Image.open(ICON) as icon:
        icon.load()
        found = problems(icon)
    relative = ICON.relative_to(ROOT).as_posix()
    for problem in found:
        print(f"{relative} {problem}")
    if not found:
        print(f"{relative}: {SIDE}x{SIDE}, no alpha channel, every pixel opaque")
    return 1 if found else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="only check the committed icon")
    if not parser.parse_args().check:
        draw()
    return check()


if __name__ == "__main__":
    sys.exit(main())
