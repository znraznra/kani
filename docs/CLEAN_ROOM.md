# Clean-room policy

Kani is designed from our own workflow requirements. To keep it that way:

**Do**
- Describe features in our own words in `docs/` before implementing them.
- Use the same *conventions* every editor uses: media panel, preview, inspector,
  multi-track timeline, standard shortcuts. These are functional, not expressive.
- Use open formats (`.cube` LUTs, SVG, Lottie, OFL fonts) and our own pack formats.
- Record the source and license of every third-party asset in `ASSET_LEDGER.md`
  *before* it is added to the repo.

**Don't**
- Decompile, disassemble, or inspect the internals of any proprietary editor.
- Copy or trace its icons, logo, name, colour palette, pixel-level layout,
  copy text, templates, effect packs, sticker packs, fonts or file formats.
- Import assets whose license forbids redistribution inside another editor
  (many "free" stock sites). Fetch those on demand via their APIs instead.

This document is a working policy, not legal advice. Get a lawyer's opinion
before commercial distribution.
