# Brewery Controller

Embedded Rust workspace for an ESP32-C3 that reads BrewTools density/temperature sensors over a CAN bus and publishes readings to MQTT over WiFi.

## Architecture

```
crates/
  core/              # no_std library shared by all firmware crates
  controller/        # main firmware: listens + probes sensors, publishes to MQTT
  test-can-devices/  # test bench: fakes a density sensor on a second ESP32-C3
```

### `crates/core` — `brewery-core`

Pure `no_std` crate with no HAL dependency. Contains:

- `can_protocol` — all BrewTools CAN constants (`NODE_TYPE_*`, `MSG_TYPE_*`, `ACK_TYPE_*`), the `CanId` struct (pack/unpack a 29-bit extended CAN ID), and `encode_float` / `decode_float` / `encode_uint32` / `decode_uint32` helpers.

Add new shared protocol logic here; keep it free of esp-hal types.

### `crates/controller` — main firmware

Six Embassy async tasks:

| Task | File | Role |
|---|---|---|
| `can_rx_task` | `src/can_tasks.rs` | Receives CAN frames, dispatches temperature and density readings into a channel |
| `can_tx_task` | `src/can_tasks.rs` | Every 5 min sends `MSG_TYPE_START_MEASUREMENT_CMD`; also forwards calibration commands from MQTT |
| `sensor_log_task` | `src/can_tasks.rs` | Drains the channel, logs readings, signals `TEMP_SIGNAL` / `DENSITY_SIGNAL` |
| `connection_task` | `src/wifi.rs` | Manages WiFi reconnection with 5 s backoff (spawned by `wifi_task`) |
| `net_task` | `src/wifi.rs` | Drives embassy-net's internal event loop (spawned by `wifi_task`) |
| `wifi_task` | `src/wifi.rs` | Connects to MQTT broker, publishes sensor readings and HA discovery messages |

**Shared state:** `TEMP_SIGNAL` and `DENSITY_SIGNAL` are static `Signal<CriticalSectionRawMutex, f32>` values signalled by `sensor_log_task` and awaited by `wifi_task`. The tracked node ID is set at compile time via `sensor-node-id` in `cfg.toml`.

`SensorReading` enum, the static `READINGS` channel, `CALIBRATION_CMD` channel, and `CALIBRATION_ACK` channel are defined in `src/main.rs`.

#### MQTT topics

| Topic | Content |
|---|---|
| `brewery/available` | `online` / `offline` (LWT) |
| `brewery/sensor/0/temperature` | float string, e.g. `"20.50"` (°C) |
| `brewery/sensor/0/density` | float string, e.g. `"1.0450"` (SG) |
| `brewery/sensor/0/calibrate` | write an SG float to trigger sensor calibration |
| `brewery/sensor/0/calibration_ack` | echoes the calibrated value on success |
| `homeassistant/sensor/brewery_0_temperature/config` | HA discovery payload (retained) |
| `homeassistant/sensor/brewery_0_density/config` | HA discovery payload (retained) |
| `homeassistant/number/brewery_0_calibrate/config` | HA calibration control discovery (retained) |

Discovery messages are published retained on every MQTT connect, so Home Assistant auto-discovers the sensors without manual configuration.

#### Configuration

WiFi and MQTT credentials live in `crates/controller/cfg.toml` — **not checked into git**. Copy and edit this file on each new machine:

```toml
[brewery-controller]
wifi-ssid      = "YourSSID"
wifi-password  = "YourPassword"
mqtt-host      = "192.168.1.100"
mqtt-port      = 1883
mqtt-username  = ""        # leave empty for unauthenticated
mqtt-password  = ""
sensor-node-id = 0         # BrewTools secondary node ID (0–7)
```

The build script (`build.rs`) reads `cfg.toml` via `esp-config` and injects values as `BREWERY_CONTROLLER_CONFIG_*` env vars at compile time. The schema is defined in `esp_config.yml`.

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

The esp-hal crates point to a **personal fork** (`JanOveSaltvedt/esp-hal`) rather than the upstream `esp-rs/esp-hal`. The fork adds support for pinning a WiFi connection to a specific AP by BSSID (`ClientConfiguration::with_bssid`), which upstream does not expose. This lets `connection_task` scan for all APs matching the configured SSID, pick the strongest one, and lock to its BSSID so the driver cannot silently roam to a weaker AP on a different channel.

Key pins:
- esp-hal / esp-rtos / esp-println / esp-backtrace / esp-radio / esp-alloc / esp-config: `98a0e08b0d03aa9c0d47d7ecb04beb0ea00d4f85`
- embassy-executor / embassy-time / embassy-sync / embassy-net / embassy-futures: `414780f2f635594d0b9b0d343ed22dfcb69f70ef`

## Building & Flashing

```bash
# Check (no hardware needed)
cargo check -p brewery-controller
cargo check -p test-can-devices

# Flash controller (uses espflash runner from .cargo/config.toml)
cargo run --release -p brewery-controller

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
