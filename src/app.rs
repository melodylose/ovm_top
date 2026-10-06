use std::sync::{Arc, Mutex};

use crate::collector::{net::NetRate, xentop::DomainView, xm::XmInfo};

#[derive(Debug, Default)]
pub struct App {
    pub xm_info: XmInfo,
    pub domains: Arc<Mutex<Vec<DomainView>>>,
    pub network: Arc<Mutex<Vec<NetRate>>>,
}
