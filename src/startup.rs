use anyhow::{Result, anyhow, ensure};
use objc2_foundation::NSBundle;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

pub fn available() -> bool {
    NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .ends_with(".app")
}

pub fn status() -> SMAppServiceStatus {
    if !available() {
        return SMAppServiceStatus::NotFound;
    }
    unsafe { SMAppService::mainAppService().status() }
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    ensure!(available(), "Open Nemo.app to change launch on startup");
    unsafe {
        let service = SMAppService::mainAppService();
        let current = service.status();
        if enabled {
            if current != SMAppServiceStatus::Enabled
                && current != SMAppServiceStatus::RequiresApproval
                && let Err(error) = service.registerAndReturnError()
                && service.status() != SMAppServiceStatus::RequiresApproval
            {
                return Err(anyhow!("{error}"));
            }
            if service.status() == SMAppServiceStatus::RequiresApproval {
                SMAppService::openSystemSettingsLoginItems();
            }
            ensure!(
                matches!(
                    service.status(),
                    SMAppServiceStatus::Enabled | SMAppServiceStatus::RequiresApproval
                ),
                "macOS did not register Nemo as a login item"
            );
        } else if current != SMAppServiceStatus::NotRegistered {
            service
                .unregisterAndReturnError()
                .map_err(|error| anyhow!("{error}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn unbundled_commands_cannot_register_a_login_item() {
        for enabled in [true, false] {
            assert_eq!(
                super::set_enabled(enabled).unwrap_err().to_string(),
                "Open Nemo.app to change launch on startup"
            );
        }
        assert_eq!(
            super::status(),
            objc2_service_management::SMAppServiceStatus::NotFound
        );
    }
}
