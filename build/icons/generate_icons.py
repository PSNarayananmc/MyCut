#!/usr/bin/env python3
"""Generate MyCut brand icons: an SVG logo plus PNG exports for the .deb
and AppImage packages (hicolor sizes). Deterministic, no network."""

import os
from cairosvg import svg2png

HERE = os.path.dirname(os.path.abspath(__file__))

SVG = """<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#1b2027"/>
      <stop offset="1" stop-color="#0e1116"/>
    </linearGradient>
    <linearGradient id="accent" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#4ade80"/>
      <stop offset="1" stop-color="#22d3ee"/>
    </linearGradient>
  </defs>
  <rect x="8" y="8" width="240" height="240" rx="52" fill="url(#bg)" stroke="#2c3440" stroke-width="4"/>
  <!-- film strip notches -->
  <g fill="#39424f">
    <rect x="30" y="34" width="18" height="12" rx="3"/>
    <rect x="30" y="70" width="18" height="12" rx="3"/>
    <rect x="30" y="106" width="18" height="12" rx="3"/>
    <rect x="30" y="142" width="18" height="12" rx="3"/>
    <rect x="30" y="178" width="18" height="12" rx="3"/>
    <rect x="30" y="214" width="18" height="12" rx="3"/>
  </g>
  <!-- play wedge -->
  <path d="M 96 76 L 176 128 L 96 180 Z" fill="url(#accent)"/>
  <!-- cut line + scissors hint -->
  <line x1="96" y1="208" x2="196" y2="48" stroke="#e8eaf0" stroke-width="10" stroke-linecap="round" stroke-dasharray="26 14"/>
  <circle cx="88" cy="220" r="14" fill="none" stroke="#e8eaf0" stroke-width="10"/>
</svg>
"""

SIZES = [512, 256, 128, 64, 48, 32, 16]

def main() -> None:
    svg_path = os.path.join(HERE, "mycut.svg")
    with open(svg_path, "w") as f:
        f.write(SVG)
    for size in SIZES:
        out = os.path.join(HERE, f"mycut-{size}.png")
        svg2png(bytestring=SVG.encode(), write_to=out, output_width=size, output_height=size)
        print("wrote", out)

if __name__ == "__main__":
    main()
