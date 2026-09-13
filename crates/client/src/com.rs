pub(crate) struct Com {
    initialized: bool,
}

impl Com {
    pub(crate) fn enter() -> Self {
        Self {
            initialized: wasapi::initialize_mta().is_ok(),
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.initialized {
            wasapi::deinitialize();
        }
    }
}
