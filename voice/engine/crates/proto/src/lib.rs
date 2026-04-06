pub mod stt {
    include!(concat!(env!("OUT_DIR"), "/stt.rs"));
    include!(concat!(env!("OUT_DIR"), "/stt.serde.rs"));
}

pub mod agent {
    include!(concat!(env!("OUT_DIR"), "/agent.rs"));
    include!(concat!(env!("OUT_DIR"), "/agent.serde.rs"));
}
