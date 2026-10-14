//! Windows ownership boundaries for panel processes and private state.
//! Agents start suspended, enter their own kill-on-close job, then resume.
use std::{fs::File, os::windows::io::AsRawHandle, path::Path, ptr, time::Duration};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE},
    Security::{self, Authorization::*, *},
    Storage::FileSystem::*,
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn failure(message: &str) -> String {
    format!("{message}: {}", std::io::Error::last_os_error())
}

pub(super) struct ProcessJob(Handle);
impl ProcessJob {
    pub async fn spawn(
        command: &mut tokio::process::Command,
    ) -> Result<(tokio::process::Child, Self), String> {
        let job = Self::new()?;
        Self::prepare(command.as_std_mut());
        let mut child = command
            .spawn()
            .map_err(|_| "Cannot start the selected Windows agent executable")?;
        if let Err(error) = job.attach(&child) {
            let _ = job.terminate();
            let _ = child.kill().await;
            let _ = child.wait().await;
            job.confirm_empty().await?;
            return Err(error);
        }
        Ok((child, job))
    }

    pub fn new() -> Result<Self, String> {
        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle == 0 {
                return Err(failure("Cannot create an owned agent job"));
            }
            let job = Self(Handle(handle));
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(failure("Cannot protect the agent process tree"));
            }
            Ok(job)
        }
    }
    pub fn prepare(command: &mut std::process::Command) {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    /// Assignment happens before any agent code can create a child process.
    pub fn attach(&self, child: &tokio::process::Child) -> Result<(), String> {
        let handle = child
            .raw_handle()
            .ok_or("Agent process handle is unavailable")? as HANDLE;
        let pid = child.id().ok_or("Agent process identity is unavailable")?;
        unsafe {
            if AssignProcessToJobObject(self.0 .0, handle) == 0 {
                return Err(failure("Cannot own the agent process tree"));
            }
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(failure("Cannot find the suspended agent thread"));
            }
            let snapshot = Handle(snapshot);
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut found = Thread32First(snapshot.0, &mut entry) != 0;
            while found {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if thread == 0 {
                        return Err(failure("Cannot resume the owned agent thread"));
                    }
                    let thread = Handle(thread);
                    if ResumeThread(thread.0) == u32::MAX {
                        return Err(failure("Cannot resume the owned agent"));
                    }
                    return Ok(());
                }
                found = Thread32Next(snapshot.0, &mut entry) != 0;
            }
        }
        Err("Suspended agent thread was not found; the agent was not started".into())
    }
    pub fn terminate(&self) -> Result<(), String> {
        unsafe {
            if TerminateJobObject(self.0 .0, 1) == 0 {
                return Err(failure("Agent process-tree termination is unconfirmed"));
            }
        }
        Ok(())
    }
    pub async fn confirm_empty(&self) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                unsafe { std::mem::zeroed() };
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0 .0,
                    JobObjectBasicAccountingInformation,
                    &mut accounting as *mut _ as _,
                    std::mem::size_of_val(&accounting) as u32,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(failure(
                    "Cannot confirm the owned agent process tree exited",
                ));
            }
            if accounting.ActiveProcesses == 0 {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("Owned agent job still has running processes".into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn token_sid(class: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>, String> {
    unsafe {
        let mut handle = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) == 0 {
            return Err(failure("Cannot identify the current Windows user"));
        }
        let token = Handle(handle);
        let mut needed = 0;
        GetTokenInformation(token.0, class, ptr::null_mut(), 0, &mut needed);
        if needed == 0 || needed > 65536 {
            return Err("Invalid Windows token identity size".into());
        }
        let mut bytes = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(token.0, class, bytes.as_mut_ptr() as _, needed, &mut needed) == 0 {
            return Err(failure("Cannot read the current Windows user identity"));
        }
        let sid = if class == TokenUser {
            (*(bytes.as_ptr() as *const TOKEN_USER)).User.Sid
        } else {
            (*(bytes.as_ptr() as *const TOKEN_OWNER)).Owner
        };
        let length = GetLengthSid(sid);
        let mut result = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        if length == 0 || CopySid(length, result.as_mut_ptr() as _, sid) == 0 {
            return Err(failure("Cannot retain the current Windows user identity"));
        }
        Ok(result)
    }
}
fn info(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION, String> {
    let mut info = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(failure("Cannot identify the open Windows file"));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("Panel state cannot be a Windows reparse point".into());
    }
    Ok(info)
}
pub(super) fn identity(file: &File) -> Result<(u64, u64), String> {
    let i = info(file)?;
    Ok((
        i.dwVolumeSerialNumber as u64,
        ((i.nFileIndexHigh as u64) << 32) | i.nFileIndexLow as u64,
    ))
}
pub(super) fn regular(file: &File) -> Result<(), String> {
    let i = info(file)?;
    if i.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 || i.nNumberOfLinks != 1 {
        return Err("Panel file must be a regular, single-link Windows file".into());
    }
    Ok(())
}
pub(super) fn open_options(options: &mut std::fs::OpenOptions, private: bool) {
    use std::os::windows::fs::OpenOptionsExt;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    if private {
        options.access_mode(
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER,
        );
    }
}
/// Set a protected owner-only DACL through the open handle, never a path alias.
/// SYSTEM is retained for OS maintenance, equivalent to privileged Unix access.
pub(super) fn protect(file: &File) -> Result<(), String> {
    info(file)?;
    let user = token_sid(TokenUser)?;
    let default_owner = token_sid(TokenOwner)?;
    unsafe {
        let mut owner = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let result = GetSecurityInfo(
            file.as_raw_handle() as _,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        );
        if result != 0 {
            return Err(format!(
                "Cannot verify Windows panel-file ownership ({result})"
            ));
        }
        let _original = Local(descriptor);
        if owner.is_null()
            || (EqualSid(owner, user.as_ptr() as _) == 0
                && EqualSid(owner, default_owner.as_ptr() as _) == 0)
        {
            return Err("Panel state belongs to another Windows user".into());
        }
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(user.as_ptr() as _, &mut text) == 0 {
            return Err(failure("Cannot encode the Windows user identity"));
        }
        let _text = Local(text as _);
        let mut length = 0;
        while length < 256 && *text.add(length) != 0 {
            length += 1;
        }
        if length == 256 {
            return Err("Windows user identity is oversized".into());
        }
        let sid = String::from_utf16(std::slice::from_raw_parts(text, length))
            .map_err(|_| "Invalid Windows user identity")?;
        let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut new_sd = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut new_sd,
            ptr::null_mut(),
        ) == 0
        {
            return Err(failure("Cannot build a private Windows ACL"));
        }
        let _new_sd = Local(new_sd);
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = ptr::null_mut();
        if Security::GetSecurityDescriptorDacl(new_sd, &mut present, &mut dacl, &mut defaulted) == 0
            || present == 0
            || dacl.is_null()
        {
            return Err("Private Windows ACL is missing".into());
        }
        let result = SetSecurityInfo(
            file.as_raw_handle() as _,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null(),
        );
        if result != 0 {
            return Err(format!("Cannot protect Windows panel state ({result})"));
        }
    }
    verify_private(file)
}
pub(super) fn protect_directory(path: &Path) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    open_options(&mut options, true);
    let file = options
        .open(path)
        .map_err(|_| "Cannot open private Windows directory")?;
    if !file
        .metadata()
        .map_err(|_| "Cannot inspect private Windows directory")?
        .is_dir()
    {
        return Err("Windows panel store is not a directory".into());
    }
    protect(&file)
}
/// Canonical parents provide the extended Win32 prefix even for a new leaf.
/// std filesystem APIs already do this; raw MoveFileExW does not.
fn extended_file_path(path: &Path) -> Result<std::path::PathBuf, String> {
    let parent = path.parent().ok_or("Windows state path has no parent")?;
    let leaf = path
        .file_name()
        .ok_or("Windows state path has no filename")?;
    Ok(std::fs::canonicalize(parent)
        .map_err(|_| "Cannot resolve Windows state directory")?
        .join(leaf))
}

pub(super) fn replace(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    let source = extended_file_path(source)?;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination = extended_file_path(destination)?;
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(failure("Cannot atomically replace Windows panel state"));
    }
    Ok(())
}

/// Preserve an existing configuration's ACL and owner on atomic replacement.
pub(super) fn copy_permissions(source: &Path, destination: &File) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    open_options(&mut options, false);
    let original = options
        .open(source)
        .map_err(|_| "Cannot open Windows configuration permissions")?;
    regular(&original)?;
    unsafe {
        let mut owner = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut sd = ptr::null_mut();
        let result = GetSecurityInfo(
            original.as_raw_handle() as _,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut sd,
        );
        if result != 0 {
            return Err(format!(
                "Cannot read Windows configuration permissions ({result})"
            ));
        }
        let _sd = Local(sd);
        let user = token_sid(TokenUser)?;
        let default_owner = token_sid(TokenOwner)?;
        if owner.is_null()
            || (EqualSid(owner, user.as_ptr() as _) == 0
                && EqualSid(owner, default_owner.as_ptr() as _) == 0)
        {
            return Err("Configuration belongs to another Windows user".into());
        }
        let mut control = 0;
        let mut revision = 0;
        if GetSecurityDescriptorControl(sd, &mut control, &mut revision) == 0 {
            return Err(failure(
                "Cannot inspect Windows configuration ACL inheritance",
            ));
        }
        let inheritance = if control & SE_DACL_PROTECTED != 0 {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        let result = SetSecurityInfo(
            destination.as_raw_handle() as _,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | inheritance,
            owner,
            ptr::null_mut(),
            dacl,
            ptr::null(),
        );
        if result != 0 {
            return Err(format!(
                "Cannot preserve Windows configuration permissions ({result})"
            ));
        }
    }
    Ok(())
}

pub(super) fn append_options(options: &mut std::fs::OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    open_options(options, false);
    options.access_mode(FILE_GENERIC_READ | FILE_APPEND_DATA | READ_CONTROL | WRITE_DAC);
}

pub(super) fn verify_private(file: &File) -> Result<(), String> {
    let user = token_sid(TokenUser)?;
    unsafe {
        let mut sd = ptr::null_mut();
        let mut owner = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let result = GetSecurityInfo(
            file.as_raw_handle() as _,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut sd,
        );
        if result != 0 {
            return Err(format!("Windows private ACL readback failed ({result})"));
        }
        let _sd = Local(sd);
        let default_owner = token_sid(TokenOwner)?;
        if owner.is_null()
            || (EqualSid(owner, user.as_ptr() as _) == 0
                && EqualSid(owner, default_owner.as_ptr() as _) == 0)
        {
            return Err("Panel state belongs to another Windows user".into());
        }

        let mut control = 0;
        let mut revision = 0;
        if GetSecurityDescriptorControl(sd, &mut control, &mut revision) == 0
            || control & SE_DACL_PROTECTED == 0
            || dacl.is_null()
        {
            return Err("Windows private ACL is not protected".into());
        }
        let mut details: ACL_SIZE_INFORMATION = std::mem::zeroed();
        if GetAclInformation(
            dacl,
            &mut details as *mut _ as _,
            std::mem::size_of_val(&details) as u32,
            AclSizeInformation,
        ) == 0
            || details.AceCount != 2
        {
            return Err("Windows private ACL has unexpected entries".into());
        }
        let mut has_user = false;
        let mut has_system = false;
        for index in 0..details.AceCount {
            let mut entry = ptr::null_mut();
            if GetAce(dacl, index, &mut entry) == 0 || entry.is_null() {
                return Err("Cannot verify Windows private ACL entry".into());
            }
            let ace = &*(entry as *const ACCESS_ALLOWED_ACE);
            if ace.Header.AceType != 0 || ace.Mask & FILE_ALL_ACCESS != FILE_ALL_ACCESS {
                return Err("Windows private ACL has unexpected permissions".into());
            }
            let sid = ptr::addr_of!(ace.SidStart) as _;
            let is_user = EqualSid(sid, user.as_ptr() as _) != 0;
            let is_system = IsWellKnownSid(sid, WinLocalSystemSid) != 0;
            has_user |= is_user;
            has_system |= is_system;
            if !is_user && !is_system {
                return Err("Windows private ACL grants access to another principal".into());
            }
        }
        if !has_user || !has_system {
            return Err("Windows private ACL is missing its expected principals".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_state_has_an_owner_only_acl_and_rejects_hard_links() {
        let root = tempfile::tempdir().unwrap();
        protect_directory(root.path()).unwrap();
        let path = root.path().join("fixture.json");
        super::super::config_transaction::atomic_write(&path, b"synthetic", None).unwrap();
        let file = super::super::config_transaction::private_open(&path, false).unwrap();
        verify_private(&file).unwrap();
        drop(file);
        std::fs::hard_link(&path, root.path().join("alias.json")).unwrap();
        assert!(super::super::config_transaction::private_open(&path, false).is_err());
    }
    #[test]
    fn atomic_replacement_supports_long_skill_snapshot_paths() {
        let root = tempfile::tempdir().unwrap();
        let mut folder = root.path().to_path_buf();
        for _ in 0..6 {
            folder = folder.join("long-private-skill-snapshot-directory");
            super::super::config_transaction::private_tree(&folder).unwrap();
        }
        let path = folder.join("SKILL.md");
        assert!(path.as_os_str().len() > 260);
        super::super::config_transaction::atomic_write(&path, b"first", None).unwrap();
        super::super::config_transaction::atomic_write(&path, b"replacement", None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }
    #[test]
    fn read_only_privacy_check_rejects_inherited_acl_without_modifying_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("unsafe.json");
        std::fs::write(&path, b"synthetic").unwrap();
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        open_options(&mut options, false);
        let file = options.open(&path).unwrap();
        assert!(verify_private(&file).is_err());
        drop(file);
        let protected = super::super::config_transaction::private_open(&path, false).unwrap();
        verify_private(&protected).unwrap();
    }
    #[tokio::test]
    async fn job_contains_descendants_before_the_leader_runs_and_confirms_their_exit() {
        let root = tempfile::tempdir().unwrap();
        let proof = root.path().join("descendant.pid");
        let mut command = tokio::process::Command::new("python");
        command.args(["-c", "import subprocess,sys,time; p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)']); open(sys.argv[1],'w').write(str(p.pid)); time.sleep(120)"]).arg(&proof)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        let (mut child, job) = ProcessJob::spawn(&mut command).await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let pid: u32 = loop {
            if let Ok(raw) = std::fs::read_to_string(&proof) {
                if let Ok(pid) = raw.parse() {
                    break pid;
                }
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let descendant = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION
                    | windows_sys::Win32::System::Threading::SYNCHRONIZATION_SYNCHRONIZE,
                0,
                pid,
            )
        };
        assert_ne!(descendant, 0);
        let descendant = Handle(descendant);
        job.terminate().unwrap();
        child.wait().await.unwrap();
        job.confirm_empty().await.unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.0, 1000) },
            windows_sys::Win32::Foundation::WAIT_OBJECT_0
        );
    }
}
