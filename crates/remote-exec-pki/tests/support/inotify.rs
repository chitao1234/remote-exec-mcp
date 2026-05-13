use std::{
    ffi::CString,
    os::raw::{c_char, c_int, c_void},
    path::Path,
};

const IN_CLOSE_WRITE: u32 = 0x0000_0008;
const IN_CREATE: u32 = 0x0000_0100;
const IN_DELETE: u32 = 0x0000_0200;
const IN_MOVED_FROM: u32 = 0x0000_0040;
const IN_MOVED_TO: u32 = 0x0000_0080;
const IN_ATTRIB: u32 = 0x0000_0004;
const IN_NONBLOCK: c_int = 0x0000_0800;
const IN_CLOEXEC: c_int = 0x0008_0000;
const EVENT_HEADER_LEN: usize = 16;

unsafe extern "C" {
    fn inotify_init1(flags: c_int) -> c_int;
    fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: u32) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
}

pub struct DirectoryWatch {
    fd: c_int,
}

impl DirectoryWatch {
    pub fn new(path: &Path) -> Self {
        let path = CString::new(path.as_os_str().to_string_lossy().as_bytes())
            .expect("watch path contains no NUL");
        let fd = unsafe { inotify_init1(IN_NONBLOCK | IN_CLOEXEC) };
        assert!(fd >= 0, "inotify_init1 failed");

        let mask = IN_CLOSE_WRITE | IN_CREATE | IN_DELETE | IN_MOVED_FROM | IN_MOVED_TO | IN_ATTRIB;
        let watch = unsafe { inotify_add_watch(fd, path.as_ptr(), mask) };
        assert!(watch >= 0, "inotify_add_watch failed");

        Self { fd }
    }

    pub fn saw_delete_for(&self, expected_name: &str) -> bool {
        self.events()
            .into_iter()
            .any(|event| event.name == expected_name && (event.mask & IN_DELETE) != 0)
    }

    fn events(&self) -> Vec<Event> {
        let mut buffer = [0_u8; 4096];
        let size = unsafe { read(self.fd, buffer.as_mut_ptr().cast::<c_void>(), buffer.len()) };
        if size <= 0 {
            return Vec::new();
        }

        let mut offset = 0_usize;
        let mut events = Vec::new();
        let size = size as usize;
        while offset + EVENT_HEADER_LEN <= size {
            let mask = u32::from_ne_bytes(
                buffer[offset + 4..offset + 8]
                    .try_into()
                    .expect("mask bytes"),
            );
            let name_len = u32::from_ne_bytes(
                buffer[offset + 12..offset + 16]
                    .try_into()
                    .expect("name length bytes"),
            ) as usize;
            let name_start = offset + EVENT_HEADER_LEN;
            let name_end = name_start + name_len;
            if name_end > size {
                break;
            }
            let raw_name = &buffer[name_start..name_end];
            let nul = raw_name
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(raw_name.len());
            let name = String::from_utf8_lossy(&raw_name[..nul]).into_owned();
            events.push(Event { mask, name });
            offset = name_end;
        }
        events
    }
}

impl Drop for DirectoryWatch {
    fn drop(&mut self) {
        unsafe {
            let _ = close(self.fd);
        }
    }
}

struct Event {
    mask: u32,
    name: String,
}
