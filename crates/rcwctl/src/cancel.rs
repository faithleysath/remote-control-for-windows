use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{bail, Result};

pub(crate) type CancelFlag = Arc<AtomicBool>;

pub(crate) fn new_cancel_flag() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

pub(crate) fn request_cancel(cancel: &CancelFlag) {
    cancel.store(true, Ordering::SeqCst);
}

pub(crate) fn is_cancelled(cancel: &CancelFlag) -> bool {
    cancel.load(Ordering::SeqCst)
}

pub(crate) fn bail_if_cancelled(cancel: Option<&CancelFlag>) -> Result<()> {
    if cancel.map(is_cancelled).unwrap_or(false) {
        bail!("operation cancelled");
    }
    Ok(())
}
