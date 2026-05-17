use crate::data_type::{DataType, HidKbStateSubtype};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HidKbStateEvent {
    Layer(u8),
    Lang(u8),
    MacMode(u8),
    RuenLayout(u8),
}

pub fn parse(data: &[u8]) -> Option<HidKbStateEvent> {
    if data.len() < 3 || data[0] != DataType::HidKbState as u8 {
        return None;
    }
    let subtype = data[1];
    let value = data[2];
    if subtype == HidKbStateSubtype::Layer as u8 {
        Some(HidKbStateEvent::Layer(value))
    } else if subtype == HidKbStateSubtype::Lang as u8 {
        Some(HidKbStateEvent::Lang(value))
    } else if subtype == HidKbStateSubtype::MacMode as u8 {
        Some(HidKbStateEvent::MacMode(value))
    } else if subtype == HidKbStateSubtype::RuenLayout as u8 {
        Some(HidKbStateEvent::RuenLayout(value))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(subtype: u8, value: u8) -> Vec<u8> {
        vec![DataType::HidKbState as u8, subtype, value]
    }

    #[test]
    fn parses_layer() {
        assert_eq!(parse(&frame(1, 7)), Some(HidKbStateEvent::Layer(7)));
    }

    #[test]
    fn parses_lang() {
        assert_eq!(parse(&frame(2, 1)), Some(HidKbStateEvent::Lang(1)));
    }

    #[test]
    fn parses_mac_mode() {
        assert_eq!(parse(&frame(3, 0)), Some(HidKbStateEvent::MacMode(0)));
    }

    #[test]
    fn parses_ruen_layout() {
        assert_eq!(parse(&frame(4, 1)), Some(HidKbStateEvent::RuenLayout(1)));
    }

    #[test]
    fn rejects_non_hid_kb_state_first_byte() {
        let mut f = frame(1, 7);
        f[0] = 0xAA; // Time
        assert_eq!(parse(&f), None);
    }

    #[test]
    fn rejects_too_short() {
        assert_eq!(parse(&[0xDD, 1]), None);
        assert_eq!(parse(&[]), None);
    }

    #[test]
    fn rejects_unknown_subtype() {
        assert_eq!(parse(&frame(99, 0)), None);
    }
}
