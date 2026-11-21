# WgTUI-Client
WireGuard TUI for managing client connections

## Features

- **Tunnel finder** — `/` opens a search prompt in the status bar, like runetui's `/` search.
  It searches tunnel names only, not `.conf` contents. As you type, it jumps to the best match
  and highlights matching letters; `↑/↓` or `Tab` cycle through matches, `Enter` keeps the
  selected tunnel, and `Esc` cancels.
- **Single-row header** — `WGC TUI` appears in the upper-left corner on one line, with an
  initial light sweep followed by an endless wave across the brand's red gradient. The header
  always stays one row high, regardless of terminal size.
- **Esc menu** — `Esc` opens a menu with an animated banner and three entries: `OPTIONS`
  (selected by default), `HELP`, and `QUIT`. Press `Esc` inside the menu to close it. The large
  menu needs 26 rows; smaller terminals use the compact version.
- **GitHub stars** — the repository's star count appears in the upper-right corner of the header,
  after the tunnel count, as `★ 42` for
  [fir3desert/WgcTUI](https://github.com/fir3desert/WgcTUI) (`★ …` while loading, `★ —` if it
  cannot be retrieved, and `★ 1.2k` for counts over one thousand). It is fetched once per hour in
  the background with the system `curl`, without blocking the interface. If GitHub's API rate
  limit is reached, the count is retrieved from the repository's public page. The count is cached
  in `/var/cache/wgctui/stars.txt`. This is the program's only network access. The Esc menu does
  not dim that corner. Disable the feature in `OPTIONS → General → GitHub stars` (or set
  `show_stars = off`): the count is hidden and no network request is made. If the header is very
  narrow, the star count is the first item removed.
- **Options (OPTIONS)** — a runetui-style screen with tabs at the top, settings on the left, and
  a description of the selected setting on the right. `↑/↓` move through settings, `←/→` (or
  `Tab`) switch tabs, `Enter` or `Space` toggle a setting, and `Esc` goes back.

  **General**

  | Setting           | Description |
  |-------------------|-------------|
  | `GitHub stars`    | Show (ON) or hide (OFF) the star count in the header. When hidden, wgctui makes no network requests. |
  | `Update interval` | How often WireGuard status, traffic, and graph data are read, as in btop. The default is **1000 ms**; the range is **100–20000 ms**. Press `Enter` and then `+` / `-` (or use `+` / `-` directly) to change it by 100 ms up to 1000 ms, then by 500 ms. Press `Enter` or `Esc` to finish adjusting. |

  **Animations** — three independent ON/OFF switches:

  | Setting            | What it animates | When OFF |
  |--------------------|------------------|----------|
  | `Header animation` | The upper-left label (light sweep and wave). | Shows the static ` wgctui ` label on a red background. |
  | `Banner animation` | The large Esc-menu banner (light sweep and wave). | Shows the completed banner without animation. |
  | `Menu animation`   | The Esc menu's entrance (the background dims and the menu fades in). | Shows the menu immediately, without a transition. |

  Each animation works independently; disabling one does not affect the others. The
  "WORKING..." spinners are not controlled by these settings.

## Configuration

Settings are saved to `/etc/wgctui.conf` as soon as they are changed (the program always runs as
root). All options are enabled by default, and the update interval is 1000 ms:

```ini
# wgctui
update_ms = 1000
show_stars = on
header_animation = on
banner_animation = on
menu_animation = on
```

`show_stars` and animation settings accept `on` / `off` (also `true` / `false`, `yes` / `no`,
and `1` / `0`). `update_ms` must be between 100 and 20000; values outside this range are clamped
to the nearest limit. The parser is tolerant: malformed lines, unknown keys, and unrecognized
values are ignored, and missing settings keep their defaults. The legacy `animations = on|off`
key is still supported; it sets all three animations, and any newer animation keys that follow
override it.

## Performance

The render loop runs quickly (~60 fps) only while an active, visible animation is running: the
header's initial sweep, the banner animation while the large menu is open, or the menu entrance.
The header's endless wave runs at ~30 fps. With all three animations disabled, the loop returns
to its idle interval (100 ms).

WireGuard data is refreshed according to `update_ms`. At 100 ms, reads are never stacked: if one
read is still running, another is not requested until it finishes (or 5 seconds pass).

## Development

```sh
cargo build --release   # binary: target/release/wgctui
cargo test              # config, keyboard handling, loop cadence, and screens (TestBackend)
# Dump screens to text for inspection:
cargo test dump_screens -- --ignored --nocapture
```
