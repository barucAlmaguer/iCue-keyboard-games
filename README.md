# iCUE KB Games

Rust CLI games that drive Corsair keyboard lighting on macOS via OpenRGB.

## Requirements

- OpenRGB installed and running.
- OpenRGB SDK server enabled (Settings > SDK Server).

## Setup

By default the client connects to `127.0.0.1:6742`. You can override this:

```
export OPENRGB_HOST=127.0.0.1
export OPENRGB_PORT=6742
```

## Run

```
cargo run -- typing --wpm=20
```

```
cargo run -- wordle
```

Controls:
- Type the words before they expire (no Enter required)
- Backspace to correct mistakes
- ESC to quit

## Game ideas (scaffolded)

- Additional games can be registered in `src/games/mod.rs`.
