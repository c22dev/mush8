#!/usr/bin/env python3
"""Create a local-only shared-font fallback from an installed macOS font."""
from pathlib import Path
from fontTools import subset
from fontTools.ttLib import TTFont

import argparse

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=Path('/System/Library/Fonts/Supplemental/Arial Unicode.ttf'))
    parser.add_argument('--output', type=Path, default=Path('work/host-font.ttf'))
    args = parser.parse_args()
    source, dest = args.source, args.output
    font = TTFont(source)
    options = subset.Options()
    options.hinting = False
    options.notdef_glyph = True
    options.notdef_outline = True
    worker = subset.Subsetter(options=options)
    worker.populate(unicodes=range(0x10000))
    worker.subset(font)
    dest.parent.mkdir(parents=True, exist_ok=True)
    font.save(dest)
    assert dest.stat().st_size < 0x1100000
    print(f'Local shared font: {dest.stat().st_size} bytes, {len(font.getBestCmap())} characters')

if __name__ == '__main__':
    main()
