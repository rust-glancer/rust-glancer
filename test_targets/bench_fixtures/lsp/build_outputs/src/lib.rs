include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub fn read_value() -> u16 {
    generated_value()
}
