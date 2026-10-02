use std::sync::{Arc, Mutex};

use crate::collector::{net::NetRate, xentop::DomainStats, xm::XmInfo};

#[derive(Debug, Default)]
pub struct App {
    pub xm_info: XmInfo,
    pub domains: Arc<Mutex<Vec<DomainStats>>>,
    pub network: Arc<Mutex<Vec<NetRate>>>,
}
