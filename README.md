# brewtech

Embedded Rust firmware for an ESP32-C3 that bridges [BrewTools](https://www.brewtools.no/) density/temperature sensors over a CAN bus to MQTT, with automatic Home Assistant discovery.

## What it does

The controller firmware runs on an ESP32-C3, connects to a CAN bus shared with BrewTools sensors, polls them for readings every five seconds, and publishes temperature (°C) and specific gravity (SG) to an MQTT broker over WiFi. Home Assistant discovers the sensors automatically via retained discovery messages — no manual sensor configuration needed.

## Hardware

| Signal | GPIO | Notes |
|---|---|---|
| CRX (transceiver → MCU) | GPIO0 | |
| CTX (MCU → transceiver) | GPIO1 | |

- **Transceiver:** SN65HVD230 (3.3 V CAN transceiver)
- **Bus speed:** 1 Mbps
- **Termination:** 120 Ω resistor at the far end of the bus
- **Sensor power:** BrewTools sensors require 24 V DC on their 5-pin connector (+24 V, CAN H, CAN L, GND)

## Prerequisites

**Rust nightly** with the embedded RISC-V target and `espflash`:

```bash
# Install Rust if needed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add the target (rust-toolchain.toml handles the channel automatically)
rustup target add riscv32imc-unknown-none-elf

# Install the flash tool
cargo install espflash
```

## Configuration

WiFi and MQTT credentials are compiled into the firmware. Create `crates/controller/cfg.toml` (it is git-ignored):

```toml
[brewery-controller]
wifi-ssid      = "YourSSID"
wifi-password  = "YourPassword"
mqtt-host      = "192.168.1.100"   # broker IP
mqtt-port      = 1883
mqtt-username  = ""                # leave empty for unauthenticated
mqtt-password  = ""
sensor-node-id = 0                 # BrewTools secondary node ID (0–7)
```

## Building & Flashing

Always build with `--release` — dev builds are orders of magnitude slower and can cause CAN timing issues.

```bash
# Flash the main controller
cargo run-controller

# Or equivalently
cargo run --release -p brewery-controller

# Verify without hardware (type-check only)
cargo check -p brewery-controller
```

Connect the ESP32-C3 via USB before running. `espflash` will detect it, flash the firmware, and open a serial monitor automatically.

## Testing without BrewTools hardware

A second ESP32-C3 running the test firmware simulates a density sensor on the CAN bus — useful for development without physical hardware.

```bash
cargo run-test-devices
```

The test device broadcasts 20.5 °C every 500 ms and responds to measurement probes with SG 1.045.

## MQTT topics

| Topic | Direction | Content |
|---|---|---|
| `brewery/available` | publish | `online` / `offline` (Last Will) |
| `brewery/sensor/0/temperature` | publish | float string, e.g. `"20.50"` (°C) |
| `brewery/sensor/0/density` | publish | float string, e.g. `"1.0450"` (SG) |
| `brewery/sensor/0/calibrate` | subscribe | write an SG float to trigger calibration |
| `brewery/sensor/0/calibration_ack` | publish | echoes the calibrated value on success |
| `homeassistant/sensor/brewery_0_temperature/config` | publish | HA discovery payload (retained) |
| `homeassistant/sensor/brewery_0_density/config` | publish | HA discovery payload (retained) |
| `homeassistant/number/brewery_0_calibrate/config` | publish | HA calibration control discovery (retained) |

The node number in the topic matches `sensor-node-id` in `cfg.toml`.

## Home Assistant

Discovery messages are published retained on every MQTT connect. Home Assistant auto-discovers:

- **Temperature** sensor (°C)
- **Specific Gravity** sensor
- **Calibrate Density** number input (write a reference SG to recalibrate the sensor)

No manual sensor configuration in Home Assistant is required.

## Repository layout

```
crates/
  core/              # no_std shared library — CAN protocol constants and encoding helpers
  controller/        # main firmware: reads CAN bus, publishes to MQTT
  test-can-devices/  # test bench: simulates a density sensor on a second ESP32-C3
```
