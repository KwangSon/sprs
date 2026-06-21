# SPRS
sprite-rust
Auto sprite sheet generator

## Why
Godot not support packed sprite sheet.
This one convert packed sheet to godot style canvas sheet.

## How to run
cargo run -- cmiro_ud6.json

### Input

- The tool should accept only one JSON config file as input.
- The only accepted config field is `input`.
- Every other config field is rejected for now.

```json
{
  "input": "assets/cdit/CDIT1.png"
}
```

### Automatic frame detection

1. Load the source image from `input`.
2. Project the image onto the y axis by checking alpha values.
3. Split the image into row regions from the y-axis on/off runs.
4. For each detected row, project that row region onto the x axis.
5. Split each row into frame regions from the x-axis on/off runs.

In other words, non-transparent pixels define the occupied regions. Transparent gaps define the boundaries between rows and frames.
