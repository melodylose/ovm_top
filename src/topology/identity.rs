use super::snapshot::DomainIdentity;
use crate::collector::xm::XmDomain;
use std::collections::HashMap;

pub fn domain_identities(domains: &[XmDomain]) -> HashMap<u32, DomainIdentity> {
    domains
        .iter()
        .map(|domain| {
            (
                domain.id,
                DomainIdentity {
                    domid: domain.id,
                    name: domain.name.clone(),
                },
            )
        })
        .collect()
}
