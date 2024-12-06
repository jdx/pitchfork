use once_cell::sync::Lazy;

pub static SYSTEM: Lazy<sysinfo::System> = Lazy::new(|| sysinfo::System::new_all());

pub fn get_process(pid: u32) -> Option<&'static sysinfo::Process> {
    SYSTEM.process(sysinfo::Pid::from_u32(pid))
}
