# brewtech

Embedded Rust workspace for an ESP32-C3 that reads BrewTools density/temperature sensors over a CAN bus.

## Architecture

```
crates/
  core/              # no_std library shared by all firmware crates
  controller/        # main firmware: listens + probes sensors
  test-can-devices/  # test bench: fakes a density sensor on a second ESP32-C3
```

### `crates/core` — `brewtech-core`

Pure `no_std` crate with no HAL dependency. Contains:

- `can_protocol` — all BrewTools CAN constants (`NODE_TYPE_*`, `MSG_TYPE_*`, `ACK_TYPE_*`), the `CanId` struct (pack/unpack a 29-bit extended CAN ID), and `encode_float` / `decode_float` / `encode_uint32` / `decode_uint32` helpers.

Add new shared protocol logic here; keep it free of esp-hal types.

### `crates/controller` — main firmware

Three Embassy async tasks:

| Task | File | Role |
|---|---|---|
| `can_rx_task` | `src/can_tasks.rs` | Receives frames, dispatches temperature and density readings into a channel |
| `density_probe_task` | `src/can_tasks.rs` | Every 5 s sends `MSG_TYPE_START_MEASUREMENT_CMD` to all 8 node IDs |
| `sensor_log_task` | `src/can_tasks.rs` | Drains the channel and logs readings via `log::info!` |

`SensorReading` enum and the static `READINGS` channel are defined in `src/main.rs`.

### `crates/test-can-devices` — test bench firmware

Flash this to a second ESP32-C3 on the same CAN bus to test the controller without physical BrewTools hardware.

Simulates a density sensor at node ID 0:
- Broadcasts temperature (20.5 °C) every 500 ms.
- Responds to `MSG_TYPE_START_MEASUREMENT_CMD` with a density reading (1.045 SG).

## Hardware

| Signal | GPIO |
|---|---|
| CRX (transceiver → MCU) | GPIO0 |
| CTX (MCU → transceiver) | GPIO1 |

Transceiver: SN65HVD230. Bus: 1 Mbps. Termination: 120 Ω at the far end.

## CAN Protocol (BrewTools)

29-bit extended CAN IDs, bit layout (MSB → LSB):

```
[28:27] priority  [26:19] senderNodeType  [18:11] receiverNodeType
[10:8]  secondaryNodeId (sensor node 0–7)  [7:0] msgType
```

We identify as `NODE_TYPE_PLC = 8`. The controller probes `NODE_TYPE_DENSITY_SENSOR = 4`.

Message data: byte 0 is a sub-index; bytes 1–4 carry a float (little-endian) or a uint32 (big-endian).

## Dependencies

All esp-hal and embassy crates are **git-pinned** to specific commits in `Cargo.toml`. Do not change those revisions independently — they must stay in sync. The `[patch.crates-io]` section redirects any transitive crates.io pulls to the same pinned commits.

Key pins:
- esp-hal / esp-rtos / esp-println / esp-backtrace: `0c42fd92c7d0e4ed8c8e4f18630b93bcb33b3e6d`
- embassy-executor / embassy-time / embassy-sync / embassy-futures: `414780f2f635594d0b9b0d343ed22dfcb69f70ef`

## Building & Flashing

```bash
# Check (no hardware needed)
cargo check -p brewtech-controller
cargo check -p test-can-devices

# Flash controller (uses espflash runner from .cargo/config.toml)
cargo run --release -p brewtech-controller

# Flash test bench
cargo run --release -p test-can-devices
```

Always build with `--release`. The esp-hal team strongly recommends it — dev builds are orders of magnitude slower and can cause timing issues on the CAN peripheral.

## Toolchain

Nightly Rust is required (`rust-toolchain.toml`). The default build target is `riscv32imc-unknown-none-elf` (set in `.cargo/config.toml`).

rust-analyzer is configured in `rust-analyzer.toml` to analyze under the host target to avoid false `can't find crate for 'test'` errors; actual compilation still targets the embedded chip.

## Embassy Task Spawning

The task macro returns `Result<SpawnToken, SpawnError>`. Unwrap it **before** passing to `spawner.spawn()`:

```rust
// Correct
spawner.spawn(my_task(arg).unwrap());

// Wrong — type mismatch at compile time
spawner.spawn(my_task(arg)).unwrap();
```
