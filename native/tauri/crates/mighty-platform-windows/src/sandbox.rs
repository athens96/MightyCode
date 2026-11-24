//! MC-42: the Windows calls of the agent sandbox. What to change is decided by `mighty-run`'s `sandbox::windows` (pure, checked on any
//! OS); this module only reads and writes security descriptors and makes the run's token:
//!
//! - [`read_dacl`] / [`write_dacl`]: a file's or folder's access list as SDDL (`D:…`), written back with `SetNamedSecurityInfoW`, which
//!   carries inheritable entries down to what is already in a folder.
//! - [`read_label`] / [`write_label`] / [`reset_child_labels`]: its integrity label (`S:(ML;…)`), which a medium process may set at
//!   medium or lower on what it owns (no administrator rights).
//! - [`restricted_token`]: this process's token, write-restricted to the run's SIDs (plain `S-1-5-21-…` SIDs, never capability SIDs)
//!   and the logon session's, every privilege but traverse removed, at low integrity, with a default access list naming those SIDs
//!   (so the run can open what it makes itself).
//! - [`limit_ui`]: the job's UI limits (no other process's windows, clipboard, desktops, system settings or log-off).

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::null_mut;

use mighty_platform::{PlatformError, Result};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    ConvertStringSidToSidW, GetNamedSecurityInfoW, ProgressInvokeNever, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    TreeResetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACL, ACL_REVISION, CreateRestrictedToken, DACL_SECURITY_INFORMATION, DISABLE_MAX_PRIVILEGE, GetLengthSid, GetSecurityDescriptorDacl,
    GetSecurityDescriptorSacl, GetTokenInformation, InitializeAcl, LABEL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SID_AND_ATTRIBUTES, SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DEFAULT_DACL,
    TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenDefaultDacl, TokenGroups, TokenIntegrityLevel,
    TokenUser, UNPROTECTED_DACL_SECURITY_INFORMATION, WRITE_RESTRICTED,
};
use windows_sys::Win32::Storage::FileSystem::{GetVolumeInformationW, GetVolumePathNameW};
use windows_sys::Win32::System::JobObjects::{
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS,
    JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JobObjectBasicUIRestrictions, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::win::{check, wide};

/// Low integrity (`S-1-16-4096`).
const LOW_INTEGRITY: &str = "S-1-16-4096";
/// `SE_GROUP_INTEGRITY`.
const GROUP_INTEGRITY: u32 = 0x0000_0020;
/// `SE_GROUP_LOGON_ID`.
const GROUP_LOGON_ID: u32 = 0xC000_0000;
/// `FILE_PERSISTENT_ACLS` (a volume flag).
const FILE_PERSISTENT_ACLS: u32 = 0x0000_0008;

fn refused(what: &str, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Refused(format!("{what}: {error}"))
}

fn win32(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

/// A NUL-terminated wide string from Windows, freed with `LocalFree`.
///
/// # Safety
/// `text` is a NUL-terminated UTF-16 string allocated with `LocalAlloc`.
unsafe fn take_text(text: *mut u16) -> String {
    // SAFETY: the caller's NUL-terminated string, freed once here.
    unsafe {
        let length = (0..).take_while(|&at| *text.add(at) != 0).count();
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text as HLOCAL);
        value
    }
}

/// A security descriptor made from SDDL, freed on drop.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn from_sddl(sddl: &str) -> io::Result<Descriptor> {
        let text = wide(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: a NUL-terminated SDDL string; the descriptor is LocalAlloc'd and freed on drop.
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut()) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Descriptor(descriptor))
    }

    /// Its DACL (null when it has none).
    fn dacl(&self) -> *mut ACL {
        let (mut present, mut defaulted, mut acl) = (0, 0, null_mut());
        // SAFETY: a valid descriptor; the ACL points into it.
        unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut acl, &mut defaulted) };
        if present == 0 { null_mut() } else { acl }
    }

    /// Its SACL (null when it has none).
    fn sacl(&self) -> *mut ACL {
        let (mut present, mut defaulted, mut acl) = (0, 0, null_mut());
        // SAFETY: a valid descriptor; the ACL points into it.
        unsafe { GetSecurityDescriptorSacl(self.0, &mut present, &mut acl, &mut defaulted) };
        if present == 0 { null_mut() } else { acl }
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW, freed once.
        unsafe { LocalFree(self.0 as HLOCAL) };
    }
}

/// `path`'s security descriptor part `what` (`DACL_SECURITY_INFORMATION`, `LABEL_SECURITY_INFORMATION`) as SDDL.
fn read_sddl(path: &Path, what: u32) -> io::Result<String> {
    let name = wide(path);
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: a NUL-terminated path; the descriptor is LocalAlloc'd by the call and freed below.
    let code = unsafe {
        GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, what, null_mut(), null_mut(), null_mut(), null_mut(), &mut descriptor)
    };
    if code != ERROR_SUCCESS {
        return Err(win32(code));
    }
    let mut text: *mut u16 = null_mut();
    // SAFETY: the descriptor just read; the string is LocalAlloc'd and freed by take_text.
    let ok = unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, SDDL_REVISION_1, what, &mut text, null_mut()) };
    let error = io::Error::last_os_error();
    // SAFETY: the descriptor GetNamedSecurityInfoW allocated.
    unsafe { LocalFree(descriptor as HLOCAL) };
    if ok == 0 {
        return Err(error);
    }
    // SAFETY: the string the conversion allocated.
    Ok(unsafe { take_text(text) })
}

/// `path`'s access list as SDDL (`D:…`).
pub fn read_dacl(path: &Path) -> io::Result<String> {
    read_sddl(path, DACL_SECURITY_INFORMATION)
}

/// `path`'s integrity label as SDDL (`S:(ML;…)`, or empty when it has none).
pub fn read_label(path: &Path) -> io::Result<String> {
    read_sddl(path, LABEL_SECURITY_INFORMATION)
}

/// Sets `path`'s access list to `dacl` (`D:<flags><explicit entries>`): protected when its flags hold `P`, else inheriting from the
/// parent again; inheritable entries are carried down to what the folder already holds.
pub fn write_dacl(path: &Path, dacl: &str) -> io::Result<()> {
    let descriptor = Descriptor::from_sddl(dacl)?;
    let flags = dacl.strip_prefix("D:").unwrap_or("").split('(').next().unwrap_or("");
    let protection = if flags.contains('P') { PROTECTED_DACL_SECURITY_INFORMATION } else { UNPROTECTED_DACL_SECURITY_INFORMATION };
    let name = wide(path);
    // SAFETY: a NUL-terminated path and an ACL inside the live descriptor.
    let code = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | protection,
            null_mut(),
            null_mut(),
            descriptor.dacl(),
            null_mut(),
        )
    };
    if code != ERROR_SUCCESS {
        return Err(win32(code));
    }
    Ok(())
}

/// An empty ACL (no entry: no label).
fn empty_acl() -> Vec<u64> {
    let size = size_of::<ACL>() as u32;
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: a buffer at least the size of an ACL header, 8-byte aligned.
    unsafe { InitializeAcl(buffer.as_mut_ptr().cast(), size, ACL_REVISION) };
    buffer
}

/// Sets `path`'s explicit integrity label to `label` (`(ML;OICI;NW;;;LW)`), or takes it away (None).
pub fn write_label(path: &Path, label: Option<&str>) -> io::Result<()> {
    let name = wide(path);
    let set = |sacl: *const ACL| -> io::Result<()> {
        // SAFETY: a NUL-terminated path and a valid ACL.
        let code = unsafe {
            SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, LABEL_SECURITY_INFORMATION, null_mut(), null_mut(), null_mut(), sacl)
        };
        if code != ERROR_SUCCESS { Err(win32(code)) } else { Ok(()) }
    };
    match label {
        Some(ace) => {
            let descriptor = Descriptor::from_sddl(&format!("S:{ace}"))?;
            set(descriptor.sacl())
        }
        None => {
            let empty = empty_acl();
            set(empty.as_ptr().cast())
        }
    }
}

/// Sets `folder`'s label to `label` (or none) and takes every explicit label off what it holds, so all of it inherits again (the
/// undo of a low label: files a low process made carry a low label of their own).
pub fn reset_child_labels(folder: &Path, label: Option<&str>) -> io::Result<()> {
    let name = wide(folder);
    let descriptor = match label {
        Some(ace) => Some(Descriptor::from_sddl(&format!("S:{ace}"))?),
        None => None,
    };
    let empty = empty_acl();
    let sacl: *const ACL = descriptor.as_ref().map_or(empty.as_ptr().cast(), |d| d.sacl());
    // SAFETY: a NUL-terminated path and a valid ACL; no progress callback.
    let code = unsafe {
        TreeResetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            sacl,
            0,
            None,
            ProgressInvokeNever,
            std::ptr::null(),
        )
    };
    if code != ERROR_SUCCESS {
        return Err(win32(code));
    }
    Ok(())
}

/// Whether `path` lies on a volume that keeps access lists and labels (NTFS, ReFS; not FAT, not a network share's view).
pub fn labels_supported(path: &Path) -> bool {
    let name = wide(path);
    let mut volume = vec![0u16; 1024];
    let mut file_system = vec![0u16; 64];
    let mut flags = 0u32;
    // SAFETY: NUL-terminated input and buffers of the sizes given.
    unsafe {
        if GetVolumePathNameW(name.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) == 0 {
            return false;
        }
        if GetVolumeInformationW(
            volume.as_ptr(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &mut flags,
            file_system.as_mut_ptr(),
            file_system.len() as u32,
        ) == 0
        {
            return false;
        }
    }
    let end = file_system.iter().position(|&c| c == 0).unwrap_or(file_system.len());
    let kind = String::from_utf16_lossy(&file_system[..end]);
    flags & FILE_PERSISTENT_ACLS != 0 && (kind.eq_ignore_ascii_case("NTFS") || kind.eq_ignore_ascii_case("ReFS"))
}

/// A SID parsed from SDDL, freed on drop.
struct Sid(PSID);

impl Sid {
    fn parse(text: &str) -> Result<Sid> {
        let wide_text = wide(text);
        let mut sid: PSID = null_mut();
        // SAFETY: a NUL-terminated SID string; the SID is LocalAlloc'd and freed on drop.
        if unsafe { ConvertStringSidToSidW(wide_text.as_ptr(), &mut sid) } == 0 {
            return Err(refused("a sandbox SID", io::Error::last_os_error()));
        }
        Ok(Sid(sid))
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSidToSidW, freed once.
        unsafe { LocalFree(self.0 as HLOCAL) };
    }
}

/// A SID as SDDL text.
///
/// # Safety
/// `sid` points to a valid SID.
unsafe fn sid_text(sid: PSID) -> Result<String> {
    let mut text: *mut u16 = null_mut();
    // SAFETY: the caller's valid SID; the text is freed by take_text.
    unsafe {
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(refused("a SID", io::Error::last_os_error()));
        }
        Ok(take_text(text))
    }
}

/// A token handle, closed on drop.
struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: a token handle this value owns.
        unsafe { CloseHandle(self.0) };
    }
}

/// `class` of `token`, in an 8-byte aligned buffer.
fn token_information(token: HANDLE, class: i32) -> Result<Vec<u64>> {
    let mut length = 0u32;
    // SAFETY: the documented size query.
    unsafe { GetTokenInformation(token, class, null_mut(), 0, &mut length) };
    let mut buffer = vec![0u64; (length as usize).div_ceil(8).max(1)];
    // SAFETY: a buffer of at least `length` bytes.
    check(unsafe { GetTokenInformation(token, class, buffer.as_mut_ptr() as *mut c_void, length, &mut length) })?;
    Ok(buffer)
}

fn process_token(access: u32) -> Result<Token> {
    let mut token: HANDLE = null_mut();
    // SAFETY: this process's pseudo handle; the token is closed on drop.
    check(unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) })?;
    Ok(Token(token))
}

/// The logon session's SID of `token` (`S-1-5-5-x-y`).
fn logon_sid_of(token: HANDLE) -> Result<String> {
    let buffer = token_information(token, TokenGroups)?;
    // SAFETY: TOKEN_GROUPS as GetTokenInformation filled it; GroupCount entries follow.
    unsafe {
        let groups = &*(buffer.as_ptr() as *const TOKEN_GROUPS);
        let entries = std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
        for entry in entries {
            if entry.Attributes & GROUP_LOGON_ID == GROUP_LOGON_ID {
                return sid_text(entry.Sid);
            }
        }
    }
    Err(PlatformError::Refused("the token has no logon session SID".into()))
}

/// Whether the agents' pipe lets a sandboxed run's processes in (some workspace's Windows sandbox is approved). Off until the app
/// reads its change record: a machine that never approved the sandbox keeps the pipe owner-only.
static AGENT_PIPE_FOR_SANDBOX: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app says whether any workspace's Windows sandbox is approved (its change record read or written); the agents' pipe instances
/// made after this follow it.
pub fn set_agent_pipe_for_sandbox(open: bool) {
    AGENT_PIPE_FOR_SANDBOX.store(open, std::sync::atomic::Ordering::SeqCst);
}

/// Whether the agents' pipe instances are made for sandboxed runs too.
pub fn agent_pipe_for_sandbox() -> bool {
    AGENT_PIPE_FOR_SANDBOX.load(std::sync::atomic::Ordering::SeqCst)
}

/// This process's logon session SID (the agents' pipe grants it, so a sandboxed run's MCP server reaches the app).
pub fn logon_sid() -> Result<String> {
    let token = process_token(TOKEN_QUERY)?;
    logon_sid_of(token.0)
}

/// `result` with the step that failed named (an OS error keeps its kind and code in the text), so a refused token says which call.
fn step<T>(what: &str, result: Result<T>) -> Result<T> {
    result.map_err(|error| match error {
        PlatformError::Io(io) => PlatformError::Io(io::Error::new(io.kind(), format!("{what}: {io}"))),
        other => other,
    })
}

/// The token a sandboxed run starts with (see the module). `restricting` are the run's SIDs (the workspace's and the agent's
/// `S-1-5-21-…` SIDs, Everyone); each must be a plain SID a restricted token takes, not a capability SID (`S-1-15-3-…`, AppContainer
/// only).
pub(crate) fn restricted_token(restricting: &[String]) -> Result<OwnedHandle> {
    let token = step("OpenProcessToken", process_token(TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT))?;
    let logon = step("the logon session SID", logon_sid_of(token.0))?;
    let user = {
        let buffer = step("TokenUser", token_information(token.0, TokenUser))?;
        // SAFETY: TOKEN_USER as GetTokenInformation filled it.
        unsafe { sid_text((*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid)? }
    };
    let mut names: Vec<String> = restricting.to_vec();
    names.push(logon.clone());
    let sids: Vec<Sid> = names.iter().map(|n| Sid::parse(n)).collect::<Result<_>>()?;
    let entries: Vec<SID_AND_ATTRIBUTES> = sids.iter().map(|s| SID_AND_ATTRIBUTES { Sid: s.0, Attributes: 0 }).collect();
    let mut restricted: HANDLE = null_mut();
    // SAFETY: a token opened with TOKEN_DUPLICATE; `entries` points at SIDs alive until the call returns.
    step(
        "CreateRestrictedToken",
        check(unsafe {
            CreateRestrictedToken(
                token.0,
                DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED,
                0,
                null_mut(),
                0,
                null_mut(),
                entries.len() as u32,
                entries.as_ptr(),
                &mut restricted,
            )
        }),
    )?;
    // SAFETY: the token CreateRestrictedToken just made, owned here.
    let restricted = unsafe { OwnedHandle::from_raw_handle(restricted) };
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&restricted) as HANDLE;

    // Low integrity.
    let low = Sid::parse(LOW_INTEGRITY)?;
    let label = TOKEN_MANDATORY_LABEL { Label: SID_AND_ATTRIBUTES { Sid: low.0, Attributes: GROUP_INTEGRITY } };
    // SAFETY: the label and its SID live until the call returns; the size counts the SID after the struct.
    step(
        "the low integrity level",
        check(unsafe {
            SetTokenInformation(
                handle,
                TokenIntegrityLevel,
                &label as *const _ as *const c_void,
                (size_of::<TOKEN_MANDATORY_LABEL>() + GetLengthSid(low.0) as usize) as u32,
            )
        }),
    )?;

    // What the run makes without a parent to inherit from (its own process, threads, pipes, events) names the restricting SIDs too,
    // or the write-restricted check would refuse the run its own objects.
    let mut sddl = format!("D:(A;;GA;;;{user})(A;;GA;;;SY)");
    for name in names.iter().filter(|n| n.as_str() != "S-1-1-0") {
        sddl.push_str(&format!("(A;;GA;;;{name})"));
    }
    let descriptor = step("the default access list", Descriptor::from_sddl(&sddl).map_err(PlatformError::Io))?;
    let default = TOKEN_DEFAULT_DACL { DefaultDacl: descriptor.dacl() };
    // SAFETY: the ACL lives in `descriptor` until the call returns (the token copies it).
    step(
        "the default access list",
        check(unsafe {
            SetTokenInformation(handle, TokenDefaultDacl, &default as *const _ as *const c_void, size_of::<TOKEN_DEFAULT_DACL>() as u32)
        }),
    )?;
    Ok(restricted)
}

/// The job's UI limits for a sandboxed run.
pub(crate) fn limit_ui(job: HANDLE) -> Result<()> {
    let limits = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_EXITWINDOWS,
    };
    // SAFETY: the struct and its size, for the class that reads it.
    check(unsafe {
        SetInformationJobObject(
            job,
            JobObjectBasicUIRestrictions,
            &limits as *const _ as *const c_void,
            size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        )
    })
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Security::{IsTokenRestricted, TokenRestrictedSids};

    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mc-sandbox-acl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        std::fs::write(dir.join("inner/file.txt"), "x").unwrap();
        dir
    }

    #[test]
    fn a_label_is_set_read_and_taken_away() {
        let dir = folder("label");
        assert!(labels_supported(&dir), "the temporary folder is on NTFS");
        write_label(&dir, Some("(ML;OICI;NW;;;LW)")).unwrap();
        assert!(read_label(&dir).unwrap().contains("ML;OICI;NW;;;LW"));
        assert!(read_label(&dir.join("inner/file.txt")).unwrap().contains("LW"), "carried down to what was there");
        reset_child_labels(&dir, None).unwrap();
        assert!(!read_label(&dir).unwrap().contains("LW"));
        assert!(!read_label(&dir.join("inner/file.txt")).unwrap().contains("LW"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_for_a_synthetic_sid_is_added_and_taken_out() {
        let dir = folder("dacl");
        let sid = "S-1-5-21-1-2-3-4";
        let before = read_dacl(&dir).unwrap();
        let explicit: String = before.clone();
        let added = format!("{explicit}(A;OICI;0x1301bf;;;{sid})");
        write_dacl(&dir, &added).unwrap();
        assert!(read_dacl(&dir.join("inner")).unwrap().contains(sid), "inherited by the folder inside");
        write_dacl(&dir, &before).unwrap();
        assert!(!read_dacl(&dir).unwrap().contains(sid));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The token a sandboxed run gets, made as the run's would be (a workspace SID and an agent SID of mighty-run's
    /// `synthetic_sid` shape, Everyone): restricted to exactly those and the logon session's SID, at low integrity.
    #[test]
    fn the_restricted_token_is_made_without_administrator_rights() {
        let workspace = "S-1-5-21-3623811015-3361044348-30300820-1013";
        let agent = "S-1-5-21-1-2-3-4";
        let token = restricted_token(&[workspace.to_string(), agent.to_string(), "S-1-1-0".to_string()]).unwrap();
        let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&token) as HANDLE;
        // SAFETY: the token just made.
        assert_ne!(unsafe { IsTokenRestricted(handle) }, 0, "a restricted token");
        let logon = logon_sid().unwrap();
        assert!(logon.starts_with("S-1-5-5-"), "{logon}");
        let buffer = token_information(handle, TokenRestrictedSids).unwrap();
        // SAFETY: TOKEN_GROUPS as GetTokenInformation filled it; GroupCount entries follow.
        let mut restricting: Vec<String> = unsafe {
            let groups = &*(buffer.as_ptr() as *const TOKEN_GROUPS);
            std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize)
                .iter()
                .map(|entry| sid_text(entry.Sid).unwrap())
                .collect()
        };
        restricting.sort();
        let mut expected = vec![workspace.to_string(), agent.to_string(), "S-1-1-0".to_string(), logon];
        expected.sort();
        assert_eq!(restricting, expected, "restricted to the run's SIDs and the logon session's");
        let buffer = token_information(handle, TokenIntegrityLevel).unwrap();
        // SAFETY: TOKEN_MANDATORY_LABEL as GetTokenInformation filled it.
        let level = unsafe { sid_text((*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid).unwrap() };
        assert_eq!(level, LOW_INTEGRITY, "at low integrity");
    }
}
