//! Keys under HKEY_CURRENT_USER as whole trees (W9-F): C# `CurrentUserRegistry` (`Core\RenameMigration.cs`), the registry RenameMigration
//! copies `Software\MightyClaudeNative` with — a key's existence, its subkeys, its values with their type and bytes as stored (an
//! expandable string unexpanded, C#'s `DoNotExpandEnvironmentNames`), a key made, a value set — and the toast activator's keys
//! (activator.rs). Writing is the caller's to allow (`Host::may_change_machine`); tests use a key of their own, removed on drop.

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExW, RegEnumKeyExW, RegEnumValueW,
    RegOpenKeyExW, RegQueryInfoKeyW, RegSetKeyValueW,
};

fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
}

fn failed(status: u32) -> io::Error {
    io::Error::from_raw_os_error(status as i32)
}

/// An open key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key this process opened and nothing else closes.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    fn open(path: &str) -> Option<Key> {
        let path = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: a NUL-terminated name under HKCU and a place for the handle.
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) };
        (status == ERROR_SUCCESS).then_some(Key(key))
    }

    /// The longest subkey name, value name and value data (in UTF-16 units, units and bytes), for the enumerations' buffers.
    fn sizes(&self) -> io::Result<(u32, u32, u32)> {
        let (mut subkey, mut name, mut data) = (0u32, 0u32, 0u32);
        let null = std::ptr::null_mut();
        // SAFETY: an open key; the three lengths asked for, every other output null as documented.
        let status = unsafe {
            RegQueryInfoKeyW(
                self.0,
                std::ptr::null_mut(),
                null,
                null,
                null,
                &mut subkey,
                null,
                null,
                &mut name,
                &mut data,
                null,
                std::ptr::null_mut(),
            )
        };
        if status == ERROR_SUCCESS { Ok((subkey, name, data)) } else { Err(failed(status)) }
    }
}

/// One value as stored: its name (`""` the default), its `REG_*` type and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawValue {
    pub name: String,
    pub kind: u32,
    pub data: Vec<u8>,
}

/// HKEY_CURRENT_USER, or the part of it below `root` a test uses (every path given is then under `root`).
#[derive(Debug, Clone, Default)]
pub struct CurrentUserRegistry {
    root: String,
}

impl CurrentUserRegistry {
    /// HKEY_CURRENT_USER itself.
    pub fn new() -> CurrentUserRegistry {
        CurrentUserRegistry { root: String::new() }
    }

    /// Every path below `root` (a test's own key under HKCU).
    pub fn under(root: &str) -> CurrentUserRegistry {
        CurrentUserRegistry { root: root.trim_end_matches('\\').to_string() }
    }

    fn path(&self, path: &str) -> String {
        if self.root.is_empty() { path.to_string() } else { format!(r"{}\{path}", self.root) }
    }

    /// C# `OpenSubKey(path) is not null`.
    pub fn key_exists(&self, path: &str) -> bool {
        Key::open(&self.path(path)).is_some()
    }

    /// C# `GetSubKeyNames()`, none for a missing key.
    pub fn sub_key_names(&self, path: &str) -> Vec<String> {
        let Some(key) = Key::open(&self.path(path)) else { return Vec::new() };
        let Ok((longest, _, _)) = key.sizes() else { return Vec::new() };
        let mut names = Vec::new();
        for index in 0.. {
            let mut buffer = vec![0u16; longest as usize + 1];
            let mut length = buffer.len() as u32;
            let null = std::ptr::null_mut();
            // SAFETY: an open key and a buffer of `length` units; the class and time outputs null as documented.
            let status = unsafe {
                RegEnumKeyExW(key.0, index, buffer.as_mut_ptr(), &mut length, null, std::ptr::null_mut(), null, std::ptr::null_mut())
            };
            if status != ERROR_SUCCESS {
                break;
            }
            names.push(String::from_utf16_lossy(&buffer[..length as usize]));
        }
        names
    }

    /// C# `GetValueNames()` with each value's data and kind, unexpanded; none for a missing key.
    pub fn values(&self, path: &str) -> Vec<RawValue> {
        let Some(key) = Key::open(&self.path(path)) else { return Vec::new() };
        let Ok((_, longest_name, longest_data)) = key.sizes() else { return Vec::new() };
        let mut values = Vec::new();
        for index in 0.. {
            let mut name = vec![0u16; longest_name as usize + 1];
            let mut name_length = name.len() as u32;
            let mut data = vec![0u8; longest_data as usize];
            let mut data_length = data.len() as u32;
            let mut kind = 0u32;
            // SAFETY: an open key, a name buffer of `name_length` units and a data buffer of `data_length` bytes.
            let status = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_length,
                    std::ptr::null_mut(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_length,
                )
            };
            match status {
                ERROR_SUCCESS => {
                    data.truncate(data_length as usize);
                    values.push(RawValue { name: String::from_utf16_lossy(&name[..name_length as usize]), kind, data });
                }
                // The end (ERROR_NO_MORE_ITEMS), or a value that grew while it was read: the enumeration stops there.
                _ => break,
            }
        }
        values
    }

    /// C# `CreateSubKey(path).Dispose()`: the key and its parents made when missing.
    pub fn create_key(&self, path: &str) -> io::Result<()> {
        let path = wide(&self.path(path));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: a NUL-terminated name under HKCU, no class or security attributes, a place for the handle.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(failed(status));
        }
        drop(Key(key));
        Ok(())
    }

    /// C# `CreateSubKey(path).SetValue(name, value, kind)`: the key made when missing, the value written with its type and bytes.
    pub fn set_value(&self, path: &str, name: &str, kind: u32, data: &[u8]) -> io::Result<()> {
        let (path, name) = (wide(&self.path(path)), wide(name));
        // SAFETY: NUL-terminated strings and a byte buffer of the given length.
        let status = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, path.as_ptr(), name.as_ptr(), kind, data.as_ptr() as *const c_void, data.len() as u32)
        };
        if status == ERROR_SUCCESS { Ok(()) } else { Err(failed(status)) }
    }

    /// A text value (`REG_SZ`), as C#'s `SetValue(name, string)` writes it.
    pub fn set_text(&self, path: &str, name: &str, text: &str) -> io::Result<()> {
        let bytes: Vec<u8> = text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
        self.set_value(path, name, windows_sys::Win32::System::Registry::REG_SZ, &bytes)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use windows_sys::Win32::System::Registry::{REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ, RegDeleteTreeW};

    /// A key of the test's own under HKCU (never the app's, `Software\Classes` or the uninstall entry), removed however the test ends.
    pub(crate) struct TestKey(pub String);

    impl TestKey {
        pub(crate) fn new(name: &str) -> TestKey {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            TestKey(format!(r"Software\MightyCode-test-{name}-{}-{nanos}", std::process::id()))
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            let key = wide(&self.0);
            // SAFETY: a NUL-terminated key name under HKCU that only this test made.
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
        }
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    /// Every kind RenameMigration copies reads back with its type and bytes as stored (an expandable string unexpanded), subkeys by
    /// name, and a missing key reads as nothing.
    #[test]
    fn a_tree_reads_back_as_written() {
        let key = TestKey::new("tree");
        let registry = CurrentUserRegistry::under(&key.0);
        assert!(!registry.key_exists("Old") && registry.values("Old").is_empty() && registry.sub_key_names("Old").is_empty());
        let written = [
            ("", REG_SZ, utf16("default")),
            ("text", REG_SZ, utf16("한국어 text")),
            ("expand", REG_EXPAND_SZ, utf16(r"%USERPROFILE%\x")),
            ("binary", REG_BINARY, vec![0, 1, 2, 255]),
            ("dword", REG_DWORD, 7u32.to_le_bytes().to_vec()),
            ("multi", REG_MULTI_SZ, [utf16("a"), utf16("b"), vec![0, 0]].concat()),
            ("qword", REG_QWORD, (1u64 << 40).to_le_bytes().to_vec()),
        ];
        for (name, kind, data) in &written {
            registry.set_value("Old", name, *kind, data).unwrap();
        }
        registry.create_key(r"Old\Child\Deep").unwrap();
        registry.create_key(r"Old\Empty").unwrap();
        assert!(registry.key_exists("Old") && registry.key_exists(r"Old\Child\Deep"));
        let mut values = registry.values("Old");
        values.sort_by(|a, b| a.name.cmp(&b.name));
        let mut expected: Vec<RawValue> =
            written.iter().map(|(name, kind, data)| RawValue { name: name.to_string(), kind: *kind, data: data.clone() }).collect();
        expected.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(values, expected);
        let mut children = registry.sub_key_names("Old");
        children.sort();
        assert_eq!(children, ["Child", "Empty"]);
        registry.set_text("Old", "text", "again").unwrap();
        assert!(registry.values("Old").iter().any(|value| value.name == "text" && value.data == utf16("again")));
    }
}
