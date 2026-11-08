# Attribution

DarkBurn is licensed under the MIT License or the Apache License, Version 2.0, at your option
(`LICENSE-MIT`, `LICENSE-APACHE`).

DarkBurn reuses code and artwork from **SolidCraft** (https://github.com/tangency-labs/solidcraft,
fork https://github.com/phobicdotno/solidcraft), Copyright (c) 2026 SolidCraft contributors,
licensed MIT OR Apache-2.0. SolidCraft's licence and notice are kept in
`licenses/SolidCraft-LICENSE-MIT` and `licenses/SolidCraft-NOTICE`. The copied source was taken
from SolidCraft 0.40.0-rc.3, commit `be6e43955`.

Nothing in DarkBurn is taken from LightBurn: no code, icons, artwork, images or help text.

## Used as crates (by path, unchanged)

| Crate | Path |
|---|---|
| solidcraft-theme | `../solidcraft/crates/theme` |
| solidcraft-ui-text | `../solidcraft/crates/ui-text` |
| solidcraft-ui-registry | `../solidcraft/crates/ui-registry` |
| solidcraft-color | `../solidcraft/crates/color` |

These build from outside SolidCraft's workspace as they are, so none had to be vendored.

## Copied source

| DarkBurn file | SolidCraft source | Changes |
|---|---|---|
| `crates/darkburn-ui/src/ui_kit/{actions,attention,checks,choice,color,confirm,container,dash,details,disclosure,drag,fields,layout,menu,metrics,panel,recent_colors,search,settings_search,shortcuts,stages,status,table,text,text_menu,tree,value_focus,wheel,preferences_layout,preferences_small}.rs` | `crates/ui-egui/src/ui_kit/` | Test modules removed; `pub(crate)` made `pub` |
| `crates/darkburn-ui/src/ui_kit/mod.rs` | `crates/ui-egui/src/ui_kit/mod.rs` | Only the modules above |
| `crates/darkburn-ui/src/ui_kit/compat.rs` | Pieces of `ui_kit/{assistant,navigation,navigation_menu,preferences,preferences_cards,reports,history}.rs` and `keymap.rs` | Lifted helper functions and constants only |
| `crates/darkburn-ui/src/fonts.rs` | `crates/ui-egui/src/fonts.rs` | Bundled faces only; system and CJK fallbacks removed |
| `crates/darkburn-ui/src/icons.rs`, `icon_svg.rs` | `crates/ui-egui/src/` | Tests removed; only the glyph source module kept |
| `crates/darkburn-ui/src/frame.rs`, `value_step.rs`, `theme_files.rs`, `theme_options.rs` | `crates/ui-egui/src/` | Tests removed; engine unit lookup replaced by a fixed list |
| `crates/darkburn-ui/src/titlebar.rs` | `crates/ui-egui/src/titlebar.rs` (itself adapted from VectorCraft, MIT OR Apache-2.0) | Close goes straight to the viewport |
| `crates/darkburn-ui/src/blur.rs` | `crates/ui-egui/src/blur.rs` | Only the no-blur stub |
| `crates/darkburn-ui/build.rs` | `crates/ui-egui/build.rs` | Only the `icon_data` and `builtin_themes` steps |
| `apps/darkburn/src/native_frame.rs` | `apps/solidcraft/src/native_frame.rs` | Uses DarkBurn's title-bar geometry |
| `apps/darkburn/src/window_fit.rs` | `apps/solidcraft/src/window_fit.rs` | None |
| `apps/darkburn/src/startup.rs` | parts of `apps/solidcraft/src/startup.rs` | Settings check and crash counter only |
| `apps/darkburn/src/main.rs` (`PrefsWriter`) | `apps/solidcraft/src/main.rs` | Pattern only |

## Assets

| Files | Author | Source | Licence |
|---|---|---|---|
| `assets/fonts/ibm-plex-sans/*`, `assets/fonts/ibm-plex-mono/*` | IBM Corp. | IBM Plex, via SolidCraft `assets/fonts` | SIL Open Font License 1.1 (`OFL.txt` beside each font) |
| `assets/icons/ui/design/svg/**` except `db_*.svg` | SolidCraft contributors | SolidCraft `assets/icons/ui/design/svg` | MIT OR Apache-2.0 |
| `assets/icons/ui/design/{palette,aliases,references}.toml` | SolidCraft contributors | SolidCraft `assets/icons/ui/design` | MIT OR Apache-2.0 |
| `assets/icons/ui/source/*.rs` | SolidCraft contributors (caption glyphs adapted from VectorCraft) | SolidCraft `assets/icons/ui/source` | MIT OR Apache-2.0 |
| `assets/icons/ui/STYLE_GUIDE.md` | SolidCraft contributors | SolidCraft `assets/icons/ui` | MIT OR Apache-2.0 |
| `assets/styles/ui/themes/*.json` | SolidCraft contributors; palettes credited in each file's `provenance` field (Nord, Gruvbox, Solarized, Dracula, Catppuccin, Tokyo Night, Atom One Dark: MIT palettes) | SolidCraft `assets/styles/ui/themes` | MIT OR Apache-2.0 |
| `assets/icons/ui/design/svg/**/db_*.svg` | DarkBurn contributors | Original artwork drawn in SolidCraft's icon style | MIT OR Apache-2.0 |
