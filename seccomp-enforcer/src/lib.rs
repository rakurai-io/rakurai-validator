use libseccomp::*;
use nix::libc;

pub fn apply_seccomp_filter_for_file_and_network_io() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = ScmpFilterContext::new_filter(ScmpAction::Allow)?;

    // -------- File I/O --------
    let file_syscalls = [
        "open", "openat", "creat", "read", "write",
        "pread64", "pwrite64", "readv", "writev",
        "stat", "fstat", "lstat", "newfstatat", "preadv",
        "unlink", "unlinkat", "rename", "renameat",
        "mkdir", "mkdirat", "rmdir", "getdents", "getdents64"
    ];

    for name in file_syscalls {
        let sc = ScmpSyscall::from_name(name)?;
        ctx.add_rule(ScmpAction::Errno(libc::EPERM), sc)?;
    }

    // -------- Network --------
    let net_syscalls = [
        "socket", "socketpair", "bind", "listen",
        "accept", "accept4", "connect",
        "sendto", "recvfrom", "sendmsg", "recvmsg",
        "shutdown", "getsockopt", "setsockopt",
        "getpeername", "getsockname"
    ];

    for name in net_syscalls {
        let sc = ScmpSyscall::from_name(name)?;
        ctx.add_rule(ScmpAction::Errno(libc::EPERM), sc)?;
    }

    ctx.load()?;

    Ok(())
}