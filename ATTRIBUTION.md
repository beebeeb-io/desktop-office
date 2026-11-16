# Attribution

What a build linking `beebeeb-office` carries beside the notices of the three folders it is made of
(`wordcraft/`, `gridcraft/`, `deckcraft/`: each keeps its own `LICENSE-*`, `NOTICE` and `ATTRIBUTION.md`, which are not
edited here). Every licence named below is in `deny.toml`'s allow list.

| Asset | Author | Source | Licence | Used for |
|---|---|---|---|---|
| Ubuntu Light (`Ubuntu-Light.ttf`, the `UBUNTU_LIGHT` font of the `epaint_default_fonts` crate 0.37.0, linked through `deckcraft-render` → `deckcraft-fonts`) | Dalton Maag Ltd for Canonical Ltd. | https://crates.io/crates/epaint_default_fonts, https://design.ubuntu.com/font | Ubuntu Font Licence 1.0 (`Ubuntu-font-1.0`) | The slide renderer's last-resort text face, when no other font has the characters. Unmodified; no font file is copied into this repository. |
