#[cfg(not(target_os = "macos"))]
#[repr(u8)]
pub enum DataType {
    Time = 0xAA, // random value that does not conflict with VIA/VIAL, must match firmware
    Volume,
    Layout,
    MediaArtist,
    MediaTitle,

    RelayFromDevice = 0xCC,
    RelayToDevice,

    HidKbState = 0xDD,
}

#[cfg(target_os = "macos")]
#[repr(u8)]
pub enum DataType {
    Time = 0xAA, // random value that does not conflict with VIA/VIAL, must match firmware
    Volume,
    Layout,
    Spotify = 0xAE,
    Weather = 0xAF,

    RelayFromDevice = 0xCC,
    RelayToDevice,

    HidKbState = 0xDD,
}

#[repr(u8)]
pub enum RelayDataType {
    Pointing = 10,
}

#[repr(u8)]
pub enum HidKbStateSubtype {
    Layer = 1,
    Lang = 2,
    MacMode = 3,
    RuenLayout = 4,
}
