# QMK HID Host

Host component for communicating with QMK keyboards using Raw HID feature.

Requires support on keyboard side, currently is supported by [stront](https://github.com/zzeneg/stront).

## Architecture

Application is written in Rust which gives easy access to HID libraries, low-level Windows/Linux APIs and cross-platform compatibility.

## Supported platforms/providers

|              | Windows            | Linux                           | MacOS                        |
| ------------ | ------------------ | ------------------------------- | ------------------           |
| Time         | :heavy_check_mark: | :heavy_check_mark:              | :heavy_check_mark:           |
| Volume       | :heavy_check_mark: | :heavy_check_mark: (PulseAudio) | :heavy_check_mark:           |
| Input layout | :heavy_check_mark: | :heavy_check_mark: (X11)        | :heavy_check_mark:           |
| Media info   | :heavy_check_mark: | :heavy_check_mark: (D-Bus)      | :heavy_check_mark: (Spotify) |
| Relay        | :heavy_check_mark: | :heavy_check_mark:              | :heavy_check_mark:           |
| Weather      |                    |                                 | :heavy_check_mark:           |
| State (device → host) | :heavy_check_mark: | :heavy_check_mark:     | :heavy_check_mark:           |
| Stream Deck bridge    | :heavy_check_mark: | :heavy_check_mark:     | :heavy_check_mark:           |

MacOS is partially supported, as I don't own any Apple devices, feel free to raise PRs.

The host also keeps a liveness handshake with the firmware: on connect it sends an initial `HID_HELLO` packet (firmware replies with a full state resync) and then pings every 30 seconds. The `state` provider listens for `HID_KB_STATE` packets pushed back from the keyboard (current layer, language, mac mode, ru/en layout) and logs them. The optional `streamdeck` provider exposes the same stream as a local WebSocket bridge for a Stream Deck plugin.

## Relay mode (device-to-device communication) - experimental

This allows for communication between two or more devices. `qmk-hid-host` only receives information from any device and broadcasts it to all devices. The actual sending and receiving should be configured in devices' firmware, but you have to set first byte in the data array - `0xCC` for sending and `0xCD` for receiving.

Example for syncing layers between two devices:

### Data type enum (common between `qmk-hid-host` and all devices)

```c
typedef enum {
    _TIME = 0xAA, // random value that does not conflict with VIA, must match companion app
    _VOLUME,
    _LAYOUT,
    _MEDIA_ARTIST, // non-macOS host only; on macOS the host sends a single _SPOTIFY frame (0xAE)
    _MEDIA_TITLE,
    _WEATHER = 0xAF, // macOS host only

    _HID_HELLO = 0xBB, // host liveness handshake (initial = full resync, then 30s ping)

    _RELAY_FROM_DEVICE = 0xCC,
    _RELAY_TO_DEVICE,

    _HID_KB_STATE = 0xDD, // device → host: current layer/lang/macMode/ruenLayout
} hid_data_type;
```

### Source device

```c
typedef enum {
    _LAYER = 0,
} relay_data_type;

layer_state_t layer_state_set_user(layer_state_t state) {
    uint8_t data[32];
    memset(data, 0, 32);
    data[0] = _RELAY_FROM_DEVICE;
    data[1] = _LAYER;
    data[2] = get_highest_layer(state);
    raw_hid_send(data, 32);

    return state;
}
```

#### Destination device

```c
typedef enum {
    _LAYER = 0,
} relay_data_type;

void raw_hid_receive_kb(uint8_t *data, uint8_t length) {
    if (data[0] == _RELAY_TO_DEVICE) {
        switch (data[1]) {
            case _LAYER:
                layer_move(data[2]);
                break;
        }
    }
}
```

## How to run it

All files are available in [latest release](https://github.com/zzeneg/qmk-hid-host/releases/tag/latest).

### Configuration

Default configuration is set to [stront](https://github.com/zzeneg/stront). For other keyboards you need to modify the configuration file (`qmk-hid-host.json`).

- `devices` section contains a list of keyboards
  - `vendorId` - `vid` from your keyboard's `info.json`. Use `"0x0000"` as a wildcard to match any vendor. You can get it by running `qmk-hid-host -p`
  - `productId` - `pid` from your keyboard's `info.json`. Use `"0x0000"` as a wildcard to match any product. You can get it by running `qmk-hid-host -p`
  - `name` - keyboard's name (optional, visible only in logs)
  - `usage` and `usagePage` - optional, override only if `RAW_USAGE_ID` and `RAW_USAGE_PAGE` were redefined in firmware
- `layouts` - list of supported keyboard layouts (app sends layout's index, not name; on macOS use the system layout names you see in logs, e.g. `"ABC"`, `"Russian"`)
- `reconnectDelay` - delay between reconnecting attempts in milliseconds (optional, default is 5000)
- `logLevel` - optional `tracing_subscriber::EnvFilter` directive (`off` / `error` / `warn` / `info` / `debug` / `trace`, or targeted form like `qmk_hid_host=warn,hidapi=off`). Default is `info`. A malformed directive panics on start. The `RUST_LOG` environment variable is **not** read.
- `providers` - optional object that toggles individual providers. Each provider is on by default; set `{ "enabled": false }` to disable. Example: `"providers": { "media": { "enabled": false } }`. Two exceptions are **off by default**:
  - `weather` — turns on only when its entry includes a `url` (macOS only). Example: `"weather": { "url": "wttr.in/Hamburg?format=%t" }`.
  - `streamdeck` — turns on only with `{ "enabled": true }`. Optional `port` (default `6543`) configures the WebSocket bridge. Bind address is hard-coded to `127.0.0.1` by design (no auth on the wire) and cannot be overridden.

  Unknown provider names or fields are rejected with a parse error.

#### Minimal config

```json
{
  "devices": [
    {
      "vendorId": "0x0000",
      "productId": "0x0844"
    }
  ],
  "layouts": ["en"]
}
```

Configuration is read from file `qmk-hid-host.json` in the current working directory. If it is not found, then the default configuration is written to this file.
You can specify a different location for the configuration file by using `--config (-c)` command line option. For example:

```
qmk-hid-host -c $HOME/.config/qmk-hid-host/config.json
```

### Windows

#### Manual/Debug mode

1. Start `qmk-hid-host.exe`
2. If needed, edit config and restart the app

#### Silent mode

When you verified that the application works with your keyboard, you can use `qmk-hid-host.silent.exe` instead (like add it to Startup). It does not have a console or logs, and can be killed only from Task Manager.

### Linux

1. Update `udev` rules by running script (remember to update `idVendor` and `idProduct` to your values first):

   ```sh
   sudo sh -c 'echo "KERNEL==\"hidraw*\", SUBSYSTEM==\"hidraw\", ATTRS{idVendor}==\"feed\", ATTRS{idProduct}==\"0844\", MODE=\"0666\"" > /etc/udev/rules.d/99-qmkhidhost.rules'
   ```

   [More info](https://get.vial.today/manual/linux-udev.html)

2. Reconnect keyboard
3. Start `qmk-hid-host`, add it to autorun if needed

### MacOS
> [!NOTE]
> To configure the weather, you need to replace your `Hamburg` with your city (example: Cairo); here is its respective repository to see more configurations: [chubin/wttr.in](https://github.com/chubin/wttr.in) - ⛅ The right way to check the weather

1. Download `qmk-hid-host`
2. Modify `qmk-hid-host.json`
3. Add your layouts or your local weather, for example:

   ```json
   "layouts": ["ABC", "Russian"],
   "providers": {
     "weather": { "url": "wttr.in/Hamburg?format=%t" }
   }
   ```

   if you don't know what layout are installed in you system, run qmk-hid-host with the layouts listed above, change lang and look at terminal output:

   ```
   INFO qmk_hid_host::providers::layout::macos: new layout: 'ABC', layout list: ["ABC", "Russian"]
   INFO qmk_hid_host::providers::layout::macos: new layout: 'Russian', layout list: ["ABC", "Russian"]
   ```

   "new layout:" is what you need

4. start `qmk-hid-host` from directory where your `qmk-hid-host.json` is located

   Note: macOS, by default, may not locate your configuration file correctly. It's recommended to start `qmk-hid-host` with the configuration file path explicitly specified, for example:
   `./qmk-hid-host -c ~/Downloads/macos/qmk-hid-host.json`

5. If you `qmk-hid-host` stuck at `Waiting for keyboard...` there are two common mistakes:
   1. You're wrong with productId in your config. Check `qmk-hid-host -p`
   2. Close Vial app and try again

## Development

### Nix

1. `nix develop`

### Native

1. Install Rust
2. Run `cargo run`
3. If needed, edit `qmk-hid-host.json` in root folder and run again

## Changelog

- 2026-05-18 - `logLevel` is read from config; `RUST_LOG` env var is no longer honoured
- 2026-05-18 - Stream Deck WebSocket bridge (`providers.streamdeck`, protocol v2, loopback-only); split HELLO into initial (synchronous, gates connect) and periodic ping
- 2026-04-26 - move provider toggles into `providers` config section; **breaking change**: top-level `weather` field replaced with `providers.weather`; rename `hid_kb_state` provider to `state`
- 2026-04-25 - add device → host `state` provider (`HID_KB_STATE`), HELLO/PING host liveness handshake, `vendorId` with wildcard matching
- 2025-11-11 - add support for weather and spotify with MacOS
- 2024-10-03 - add support for multiple devices, restructure config
- 2024-09-15 - add MacOS support
- 2024-02-06 - add Linux support
- 2024-01-21 - remove run as windows service, add silent version instead
- 2024-01-02 - support RUST_LOG, run as windows service
- 2023-07-30 - rewritten to Rust
