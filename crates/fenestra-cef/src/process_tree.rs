use std::{
    process::{Child, Command, ExitStatus},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

pub(crate) struct ManagedChild {
    child: Child,
    group: ProcessGroup,
}

impl ManagedChild {
    pub(crate) fn new(child: Child) -> Self {
        Self {
            group: ProcessGroup::register(child.id()),
            child,
        }
    }

    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        let status = self.child.wait();
        self.group.unregister();
        status
    }

    pub(crate) fn terminate(&mut self) {
        self.group.terminate();
        for _ in 0..10 {
            if !matches!(self.child.try_wait(), Ok(None)) {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        self.group.kill();
        let _ = self.child.wait();
        self.group.unregister();
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.group.unregister();
    }
}

pub(crate) fn prepare_child_command(command: &mut Command) {
    platform::prepare_child_command(command);
}

struct ProcessGroup {
    id: u32,
    active: AtomicBool,
}

impl ProcessGroup {
    fn register(id: u32) -> Self {
        platform::register_process_group(id);
        Self {
            id,
            active: AtomicBool::new(true),
        }
    }

    fn terminate(&self) {
        if !self.active.load(Ordering::SeqCst) {
            return;
        }
        platform::terminate_process_group(self.id);
    }

    fn kill(&self) {
        if !self.active.load(Ordering::SeqCst) {
            return;
        }
        platform::kill_process_group(self.id);
    }

    fn unregister(&self) {
        if !self.active.swap(false, Ordering::SeqCst) {
            return;
        }
        platform::unregister_process_group(self.id);
    }
}

#[cfg(unix)]
mod platform {
    use std::{
        process::Command,
        sync::atomic::{AtomicBool, AtomicI32, Ordering},
    };

    const SIGINT: i32 = 2;
    const SIGKILL: i32 = 9;
    const SIGTERM: i32 = 15;
    const GROUP_CAPACITY: usize = 128;

    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static PROCESS_GROUPS: [AtomicI32; GROUP_CAPACITY] =
        [const { AtomicI32::new(0) }; GROUP_CAPACITY];

    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
        fn signal(signal: i32, handler: extern "C" fn(i32)) -> usize;
        fn _exit(status: i32) -> !;
    }

    pub(super) fn prepare_child_command(command: &mut Command) {
        use std::os::unix::process::CommandExt;

        install_signal_cleanup();
        command.process_group(0);
    }

    pub(super) fn install_signal_cleanup() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            signal(SIGINT, handle_signal);
            signal(SIGTERM, handle_signal);
        }
    }

    pub(super) fn register_process_group(id: u32) {
        let Ok(id) = i32::try_from(id) else {
            return;
        };
        if id <= 0 {
            return;
        }
        install_signal_cleanup();
        for slot in &PROCESS_GROUPS {
            if slot
                .compare_exchange(0, id, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
        }
    }

    pub(super) fn unregister_process_group(id: u32) {
        let Ok(id) = i32::try_from(id) else {
            return;
        };
        for slot in &PROCESS_GROUPS {
            let _ = slot.compare_exchange(id, 0, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    pub(super) fn terminate_process_group(id: u32) {
        send_process_group(id, SIGTERM);
    }

    pub(super) fn kill_process_group(id: u32) {
        send_process_group(id, SIGKILL);
    }

    extern "C" fn handle_signal(signal: i32) {
        for slot in &PROCESS_GROUPS {
            let id = slot.load(Ordering::SeqCst);
            if id > 0 {
                unsafe {
                    kill(-id, SIGTERM);
                    kill(-id, SIGKILL);
                }
            }
        }
        unsafe {
            _exit(128 + signal);
        }
    }

    fn send_process_group(id: u32, signal: i32) {
        let Ok(id) = i32::try_from(id) else {
            return;
        };
        if id <= 0 {
            return;
        }
        unsafe {
            kill(-id, signal);
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use std::process::Command;

    pub(super) fn prepare_child_command(_command: &mut Command) {}

    #[allow(dead_code)]
    pub(super) fn install_signal_cleanup() {}

    pub(super) fn register_process_group(_id: u32) {}

    pub(super) fn unregister_process_group(_id: u32) {}

    pub(super) fn terminate_process_group(_id: u32) {}

    pub(super) fn kill_process_group(_id: u32) {}
}
