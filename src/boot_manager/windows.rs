//! Keep Windows boot updates in their original registry root. Automatic path
//! repair must preserve Task Manager approval, including disabled registrations.

use auto_launcher::{AutoLaunch, WindowsEnableMode};
use std::io;
use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};

const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const APP_NAME: &str = "pitchfork";

#[derive(Clone, Copy)]
enum RegistryRoot {
    Machine,
    User,
}

impl RegistryRoot {
    fn key(self) -> &'static Key {
        match self {
            Self::Machine => LOCAL_MACHINE,
            Self::User => CURRENT_USER,
        }
    }

    fn enable_mode(self) -> WindowsEnableMode {
        match self {
            Self::Machine => WindowsEnableMode::System,
            Self::User => WindowsEnableMode::CurrentUser,
        }
    }

    fn registered_command(self) -> io::Result<Option<String>> {
        match self
            .key()
            .open(RUN_KEY)
            .and_then(|key| key.get_string(APP_NAME))
        {
            Ok(command) => Ok(Some(command)),
            Err(error) => {
                let error = registry_error(error.into());
                if error.kind() == io::ErrorKind::NotFound {
                    return Ok(None);
                }
                Err(error)
            }
        }
    }

    /// Approval applies only to the registration in the same registry root.
    fn is_enabled(self) -> io::Result<bool> {
        if self.registered_command()?.is_none() {
            return Ok(false);
        }
        match self
            .key()
            .open(STARTUP_APPROVED_KEY)
            .and_then(|key| key.get_value(APP_NAME))
        {
            // Keep auto-launcher's decoding: a nonzero trailing timestamp
            // disables startup, while missing or short records allow it.
            Ok(value) => Ok(value.len() < 8 || value.iter().rev().take(8).all(|byte| *byte == 0)),
            Err(error) => {
                let error = registry_error(error.into());
                if error.kind() == io::ErrorKind::NotFound {
                    return Ok(true);
                }
                Err(error)
            }
        }
    }

    fn write_error(self, error: io::Error) -> io::Error {
        let error = registry_error(error);
        if matches!(self, Self::Machine) && error.kind() == io::ErrorKind::PermissionDenied {
            return io::Error::new(
                error.kind(),
                "cannot update the existing HKLM boot registration; run \
                 `pitchfork boot enable` from an elevated terminal",
            );
        }
        error
    }
}

struct Registration {
    root: RegistryRoot,
    command: String,
}

pub(super) struct WindowsLauncher {
    app_path: String,
    args: Vec<String>,
}

impl WindowsLauncher {
    pub(super) fn new(app_path: &str, args: &[String]) -> Self {
        Self {
            app_path: app_path.to_owned(),
            args: args.to_vec(),
        }
    }

    fn launcher(&self, mode: WindowsEnableMode) -> AutoLaunch {
        AutoLaunch::new(
            APP_NAME,
            &format!("\"{}\"", self.app_path),
            mode,
            &self.args,
        )
    }

    pub(super) fn enable(&self) -> auto_launcher::Result<()> {
        // Only a new registration may choose a root based on write access.
        // Updating an existing entry must never fall back to a second root.
        if let Some(registration) = Self::registration()? {
            return self.enable_in_root(registration.root);
        }
        match self.enable_in_root(RegistryRoot::Machine) {
            Err(auto_launcher::Error::Io(error))
                if error.kind() == io::ErrorKind::PermissionDenied =>
            {
                // auto-launcher writes Run before StartupApproved. If only
                // approval failed, an entry already exists: do not duplicate it.
                if Self::registration()?.is_some() {
                    return Err(error.into());
                }
                self.enable_in_root(RegistryRoot::User)
            }
            result => result,
        }
    }

    fn enable_in_root(&self, root: RegistryRoot) -> auto_launcher::Result<()> {
        self.launcher(root.enable_mode())
            .enable()
            .map_err(|error| match error {
                auto_launcher::Error::Io(error) => root.write_error(error).into(),
                error => error,
            })
    }

    pub(super) fn disable(&self) -> auto_launcher::Result<()> {
        self.launcher(WindowsEnableMode::Dynamic).disable()
    }

    pub(super) fn is_enabled(&self) -> auto_launcher::Result<bool> {
        Ok(RegistryRoot::Machine.is_enabled()? || RegistryRoot::User.is_enabled()?)
    }

    pub(super) fn get_registered_app_path(&self) -> io::Result<Option<String>> {
        Self::registration()?
            .map(|registration| executable_path(&registration.command))
            .transpose()
    }

    /// Rewrite only the selected Run value, never its StartupApproved value.
    pub(super) fn repair_if_stale(&self) -> io::Result<()> {
        let Some(registration) = Self::registration()? else {
            return Ok(());
        };
        let registered = executable_path(&registration.command)?;
        if registered == self.app_path {
            return Ok(());
        }
        let command = format!("\"{}\" {}", self.app_path, self.args.join(" "));
        registration
            .root
            .key()
            .options()
            .write()
            .open(RUN_KEY)
            .and_then(|key| key.set_string(APP_NAME, command))
            .map_err(|error| registration.root.write_error(error.into()))?;
        info!(
            "boot registration updated from '{registered}' to '{}'; Task Manager approval preserved",
            self.app_path
        );
        Ok(())
    }

    fn registration() -> io::Result<Option<Registration>> {
        for root in [RegistryRoot::Machine, RegistryRoot::User] {
            if let Some(command) = root.registered_command()? {
                return Ok(Some(Registration { root, command }));
            }
        }
        Ok(None)
    }
}

fn registry_error(error: io::Error) -> io::Error {
    // windows-registry converts HRESULTs directly into OS errors. Extract the
    // Win32 code so NotFound and PermissionDenied retain their standard kinds.
    if let Some(code) = error.raw_os_error().map(|code| code as u32)
        && code & 0xffff_0000 == 0x8007_0000
    {
        return io::Error::from_raw_os_error((code & 0xffff) as i32);
    }
    error
}

fn executable_path(command: &str) -> io::Result<String> {
    let command = command.trim_start();
    let path = if let Some(quoted) = command.strip_prefix('"') {
        quoted.split_once('"').map(|(path, _)| path)
    } else {
        // Read registrations from earlier versions so automatic repair can
        // upgrade an unquoted path containing spaces to a quoted command.
        command.split_whitespace().next()
    };
    path.filter(|path| !path.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid pitchfork boot command; run `pitchfork boot disable` followed by \
                 `pitchfork boot enable` to replace the malformed registration",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::executable_path;

    #[test]
    fn reads_quoted_and_legacy_boot_commands() {
        assert_eq!(
            executable_path(r#""C:\Program Files\Pitchfork\pitchfork.exe" supervisor run --boot"#)
                .unwrap(),
            r"C:\Program Files\Pitchfork\pitchfork.exe"
        );
        assert_eq!(
            executable_path(r"C:\tools\pitchfork.exe supervisor run --boot").unwrap(),
            r"C:\tools\pitchfork.exe"
        );
        let error = executable_path(r#""C:\unterminated"#).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("pitchfork boot disable"));
        assert!(error.to_string().contains("pitchfork boot enable"));
        assert!(executable_path("").is_err());
        assert!(executable_path(r#""" supervisor"#).is_err());
    }
}
