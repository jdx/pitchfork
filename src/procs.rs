use sysinfo::ProcessesToUpdate;

pub struct Procs {
    system: sysinfo::System,
}

impl Procs {
    pub fn new() -> Self {
        let mut procs = Self {
            system: sysinfo::System::new(),
        };
        procs.refresh_processes();
        procs
    }

    pub fn get_process(&self, pid: u32) -> Option<&sysinfo::Process> {
        self.system.process(sysinfo::Pid::from_u32(pid))
    }

    pub fn refresh_processes(&mut self) {
        self.system.refresh_processes(ProcessesToUpdate::All, true);
    }
}
