# Bundled typography

Space Grotesk is used for site headings and deterministic social preview images.
The site loads it locally through `theme/custom.css`; image generation reads the
same font file through resvg. Neither needs a remote font request.

Source: [Google Fonts](https://github.com/google/fonts/tree/main/ofl/spacegrotesk).
License: [SIL Open Font License 1.1](OFL.txt).

Keep `SpaceGrotesk.ttf` and `OFL.txt` together when updating the font. Rebuild the
docs and verify both themes and social images afterward.
