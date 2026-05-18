# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Проект

Кросс-платформенный Rust-хост, общающийся с QMK-клавиатурами через Raw HID. Шлёт на клавиатуру системные данные (время, громкость, раскладка, медиа), при необходимости ретранслирует сообщения между несколькими подключёнными устройствами и логирует на stdout входящие `HidKbState`-пакеты (текущий слой, язык, режимы).

## Build, Run, Test

- `cargo run` — dev-запуск; читает/создаёт `./qmk-hid-host.json` в cwd. Путь переопределяется через `-c <path>`.
- `cargo run -- --print-hids` (`-p`) — печатает все доступные HID-устройства и выходит; полезно для подбора `vendorId`/`productId`/`usagePage`.
- `cargo build --release` — релиз-сборка.
- `cargo make dist` — собирает артефакты в `dist/` через `Makefile.toml`. На Windows дополнительно собирает вариант с фичей `silent` (`qmk-hid-host.silent.exe`, без консоли; используется `windows_subsystem = "windows"`).
- `cargo test` — то, что гоняет CI (`.github/workflows/test.yml`). Сейчас в `src/config.rs` лежит `#[cfg(test)] mod tests` с serde-проверками схемы конфига (defaults, `deny_unknown_fields`, обязательность `weather.url`, отказ от старого top-level `weather`).
- Уровень логирования задаётся полем `logLevel` в `qmk-hid-host.json` (см. ниже). Переменная окружения `RUST_LOG` НЕ читается. Дефолт — `INFO`.
- Зависимости сборки на Linux: `./install-build-deps.sh` (libudev, pulseaudio, libdbus-1, libx11). На Linux в рантайме нужно udev-правило для hidraw — см. README.
- Альтернативно — dev-shell через `nix develop` (`flake.nix`, Linux).
- `rustfmt.toml` задаёт `max_width = 140`. Перед коммитом — `cargo fmt`.

## Архитектура

### Топология каналов
`main.rs` создаёт три Tokio-канала и подключает к ним всё остальное:
- `host_to_device_sender: broadcast::Sender<Vec<u8>>` — провайдеры рассылают кадры всем клавиатурам. Capacity 16 (см. комментарий в `main.rs` — мал ⇒ `Lagged`-дропы; ловятся `warn!` в `start_write`/`relay`/`state`).
- `device_to_host_sender: broadcast::Sender<Vec<u8>>` — клавиатуры сходятся в провайдеры-потребители (`relay`, `state`). Та же capacity 16.
- `is_connected_sender: mpsc::Sender<bool>` — потоки клавиатур сообщают о connect/disconnect; main-цикл считает и гейтит провайдеры.

### Поток клавиатуры (`keyboard.rs`)
Каждому device из конфига выдаётся отдельный OS-поток, который крутит `HidApi::new()` + `get_device_info` (matching по vendor_id/product_id/usage/usage_page; `0` — wildcard). При совпадении спавнит ещё два потока: `start_write` (подписан на `host_to_device_sender`, дописывает префиксный `0` report-ID, паддит до 32 байт, пишет в устройство) и `start_read` (читает 32-байтовые кадры, форвардит в `device_to_host_sender`). Disconnect сигналится через `is_connected: AtomicBool`; внешний цикл затем спит `reconnect_delay` и переподключается. В обоих циклах есть `sleep` 10 мс — он явно нужен, чтобы CPU usage хоста падал на порядок (коммит `0d7b3c6`); удалять нельзя.

**Initial HELLO is a hard gate**: после `start_write`/`start_read` host шлёт `HELLO_PACKET_INITIAL` синхронно через `host_to_device_sender.send()`. Если broadcast возвращает Err (writer ещё не подписался), connect-цикл **прерывается** через `write_alive=false` + `continue` — `is_connected` не флипается в `true`, провайдеры не стартуют. Цена этой жёсткости: гарантия, что прошивка успела получить INITIAL и сделала `full_sync`, прежде чем state-фреймы поедут наружу. Подключение пере-попытается в следующем витке внешнего loop.

### Trait Provider (`providers/_base.rs`)
`trait Provider { fn start(&self) -> ProviderHandle; }`. `ProviderHandle::spawn(|alive| ...)` создаёт `Arc<AtomicBool>` и поток; `handle.stop()` опускает флаг и поток выходит на следующей итерации. Каждый рабочий цикл должен периодически проверять `alive` (типично `try_recv` + короткий `sleep`). Текущие провайдеры: `time`, `volume`, `layout`, `media`, `relay`, `state`, `weather` (только macOS), `streamdeck`. `volume`/`layout`/`media` — фасад-модули, реэкспортирующие OS-specific реализацию через `#[cfg(target_os = ...)]` из `providers/<name>/{linux,windows,macos}.rs`. Включение каждого провайдера управляется секцией `providers` в конфиге (см. ниже).

### Жизненный цикл (`main.rs::start`)
`get_providers` один раз собирает список `Box<dyn Provider>` по конфигу. Затем блокирующий цикл на `is_connected_receiver`: хранит `connected_count` и `Vec<ProviderHandle>`. При любом изменении в работающем состоянии: останавливает все хендлы, спит 200 мс, заново вызывает `start()` у каждого провайдера и собирает новые хендлы — так свежеподключённая клавиатура получает заново всё текущее состояние. При `connected_count == 0` хендлов нет.

### Особенности macOS
Часть Cocoa-API (volume, layout) требует `CFRunLoopRun()` на main-потоке, поэтому на macOS `start` уезжает в спавн-поток, а `main` блокируется в `CFRunLoopRun`. `WeatherProvider` существует только на macOS (под `#[cfg(target_os = "macos")]`); включается секцией `providers.weather` в конфиге (требует `url`). На остальных ОС попытка включить `weather` логируется как `warn!` и игнорируется. `MediaProvider` на macOS реализован через Spotify (см. `providers/media/macos.rs`).

### Wire-протокол (`data_type.rs`)
Первый байт каждого 32-байтового кадра — дискриминант `DataType`. **Эти значения обязаны совпадать с enum `hid_data_type` в прошивке QMK** — комментарий в `data_type.rs` несущий.
- Не-macOS host → device: `Time=0xAA`, `Volume=0xAB`, `Layout=0xAC`, `MediaArtist=0xAD`, `MediaTitle=0xAE`.
- macOS host → device: `Time=0xAA`, `Volume=0xAB`, `Layout=0xAC`, `Spotify=0xAE`, `Weather=0xAF` (нет `MediaArtist`; вместо него — единый `Spotify`).
- `HidHello=0xBB` — host liveness ping (общий для обеих ОС). Пакет: `[0xBB, version(u8), flag(u8)]`, где `flag=1` — initial (шлётся синхронно в `keyboard.rs::connect` сразу после открытия HID-устройства; прошивка на него отвечает full resync — текущие `layer`/`lang`/`macMode`/`ruenLayout`), `flag=0` — periodic ping каждые 30с из `start_hello_pinger` (только обновляет watchdog в прошивке, без resync). Прошивка ergohaven: `keyboards/ergohaven/hid.c::process_raw_hid_data` `case _HID_HELLO`.
- Двусторонний relay: `RelayFromDevice=0xCC` (device → host), `RelayToDevice=0xCD` (host → device). `RelayProvider` форвардит только кадры, у которых первый байт — `RelayFromDevice`, переписывает его на `RelayToDevice` и шлёт в `host_to_device_sender` — кадр получают все остальные подключённые клавиатуры. Подтипы payload'а интерпретирует прошивка, хост в них не заглядывает.
- `HidKbState=0xDD` — поток с устройства на хост; разбирается в `StateProvider` (только этот дискриминант, остальные кадры провайдер игнорирует) и логируется через `tracing::info!` (виден при `logLevel: "info"` и выше). Подтипы в `HidKbStateSubtype`: `Layer=1`, `Lang=2`, `MacMode=3`, `RuenLayout=4`.

### Конфиг (`config.rs`)
Грузится один раз в `OnceLock<Config>` из `./qmk-hid-host.json` (или пути из `-c`). Если файла нет — пишется дефолтный. На корневом `Config` стоит `deny_unknown_fields`: опечатка в любом верхнем поле → паника на старте с понятной serde-ошибкой.

- `devices`: `vendor_id`/`product_id` — JSON-строки вида `"0x0844"`, `0` — wildcard. `usage`/`usage_page` дефолтятся в `0x61`/`0xff60` (QMK Raw HID).
- `layouts`: список идентификаторов раскладок — провайдер шлёт **индекс** активной раскладки, а не имя; имена платформо-зависимые (например, `"ABC"`, `"Russian"` на macOS — узнавать их по логам `INFO ... new layout: '<name>'`).
- `reconnect_delay`: мс между попытками реконнекта (default 5000).
- `logLevel`: опционально, директива `tracing_subscriber::EnvFilter` (`off`/`error`/`warn`/`info`/`debug`/`trace` или таргетная форма типа `qmk_hid_host=warn,hidapi=off`). Default — `info`. Малформная директива → panic на старте. Env-переменная `RUST_LOG` сознательно не читается. См. `main.rs::build_log_filter`.
- `providers`: опциональная секция, отключает/включает провайдеры. Каждый провайдер по умолчанию **включён**; чтобы выключить — `"<name>": { "enabled": false }`. Тип `Providers` тоже с `deny_unknown_fields` (опечатка имени → паника). Секция `weather` — единственное исключение: opt-in, требует `url` (типично `wttr.in/<city>?format=%t`); поле `enabled` опционально (default true). `WeatherProvider` `curl`-ает URL и шлёт распарсенную температуру.
- `streamdeck` — единственный после `weather` провайдер, который по умолчанию **выключен**. Поднимает WebSocket-сервер на `127.0.0.1:<port>` (default port `6543`, переопределяется через `providers.streamdeck.port`). Bind жёстко зашит в `127.0.0.1` — выставить наружу нельзя by design (нет auth, данные доверять только локальным процессам); попытка указать поле `bind` в конфиге падает на `deny_unknown_fields`. Транслирует поток `HidKbState` в JSON-фреймах. На коннекте `{"type":"hello","protocol":2,…}` + `{"type":"snapshot","values":{"<key>":{"raw":<u8>,"label?":"<str>"},…}}` (последние известные значения по подтипам — entries имеют тот же `{raw, label?}` shape, что и state-фреймы). Далее push `{"type":"state","subtype":"<key>","raw":<u8>,"label?":"<str>"}` на каждое событие. Подтипы в JSON: `layer`, `lang`, `macMode`, `ruenLayout`. Команды client → device не поддерживаются.

**Breaking change (2026-04-26):** старое верхнеуровневое поле `weather: { url }` удалено; используется `providers.weather: { url }`. Конфиги со старым форматом падают на старте.
