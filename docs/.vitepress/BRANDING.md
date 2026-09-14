# Branding assets

`docs/public/img/logo.png` is the master artwork: the red cartoon devil head
holding a golden three-tined pitchfork. Preserve its proportions and colors;
use it without rotation or brightness filters. This is raster artwork, not SVG.

After replacing the master, run `mise run render:branding` from the repository
root. It regenerates the root-level logo, legacy small logo, docs and
web UI icons, Apple touch icons, PWA icons, and legacy social card. The normal
docs build generates page-specific social cards from the same master.

The generator uses sharp for filtered downscaling and resvg for social cards.
It preserves transparent backgrounds and emits PNG favicon frames at 16, 32,
and 48 pixels inside the ICO.
Keep the PNG PWA and Apple touch exports for installation compatibility.

The master was created with the built-in image generation tool. Design brief:

> Create an original logo for Pitchfork, an open-source daemon supervisor:
> a compact cartoon red devil-head emblem with two cream horns, expressive
> asymmetric eyes, a crooked grin with one fang, and a little hand gripping
> an upright golden pitchfork with exactly three separated pointed tines.
> Bold dark outlines, simple shapes, limited red, cream, near-black and gold
> palette, transparent background, no text or scenery. Warm, mischievous
> competence; recognizable in documentation and small icons.
