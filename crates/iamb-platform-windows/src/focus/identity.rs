//! Who a process is, from the path of its executable. No system call.
//!
//! The application name is the executable's file name in lower case (`notepad.exe`): Windows
//! compares file names without regard to case, and the same program can be started under
//! either spelling, so one spelling is chosen here. The service compares exclusions with this
//! name exactly. The directory is used for one thing only, the `own` flag, and is not kept: a
//! path can contain the user's name.

use iamb_platform::AppIdentity;

/// The name of a process whose executable Windows does not tell (a protected process, one
/// that ended meanwhile). `?` cannot be part of a file name, so no executable and no
/// exclusion has this name.
pub(crate) const UNNAMED: &str = "?";

/// The directory and the file name of `path`. Both separators count, as they do for Windows.
fn split(path: &str) -> (Option<&str>, &str) {
    match path.rfind(['\\', '/']) {
        Some(at) => (Some(&path[..at]), &path[at + 1..]),
        None => (None, path),
    }
}

/// The application name for the executable at `image`.
pub(crate) fn app_name(image: &str) -> String {
    let file = split(image).1;
    if file.is_empty() {
        UNNAMED.to_owned()
    } else {
        file.to_lowercase()
    }
}

/// A directory in the form two of them are compared in: without the prefix that switches
/// path parsing off, with one kind of separator, in lower case.
fn directory_key(directory: &str) -> String {
    let plain = directory.strip_prefix(r"\\?\").unwrap_or(directory);
    plain.replace('/', "\\").to_lowercase()
}

/// True when the two executables are in the same directory.
pub(crate) fn same_directory(image: &str, other: &str) -> bool {
    match (split(image).0, split(other).0) {
        (Some(a), Some(b)) => directory_key(a) == directory_key(b),
        _ => false,
    }
}

/// The identity of process `pid`, whose executable is at `image` (`None` when Windows does
/// not tell). It is one of Iamb's own when its executable is in the directory of
/// `own_image`, the running executable.
pub(crate) fn identify(image: Option<&str>, pid: u32, own_image: Option<&str>) -> AppIdentity {
    let Some(image) = image else {
        return AppIdentity::new(UNNAMED, pid);
    };
    let identity = AppIdentity::new(app_name(image), pid);
    if own_image.is_some_and(|own| same_directory(image, own)) {
        identity.own()
    } else {
        identity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICE: &str = r"C:\Users\Ada\AppData\Local\Programs\Iamb\iamb-service.exe";

    #[test]
    fn the_name_is_the_file_name_in_lower_case() {
        for (image, name) in [
            (r"C:\Windows\System32\notepad.exe", "notepad.exe"),
            (r"C:\Program Files\Microsoft Office\EXCEL.EXE", "excel.exe"),
            ("C:/tools/Editor.exe", "editor.exe"),
            (r"\\?\C:\x\WindowsTerminal.exe", "windowsterminal.exe"),
            (r"\\server\share\ÄPFEL.EXE", "äpfel.exe"),
            ("bare.exe", "bare.exe"),
            ("no-extension", "no-extension"),
        ] {
            assert_eq!(app_name(image), name, "{image}");
        }
    }

    #[test]
    fn a_path_without_a_file_name_is_unnamed() {
        assert_eq!(app_name(""), UNNAMED);
        assert_eq!(app_name(r"C:\dir\"), UNNAMED);
        let unnamed = identify(None, 7, Some(SERVICE));
        assert_eq!(unnamed, AppIdentity::new(UNNAMED, 7));
        assert!(!unnamed.own);
    }

    #[test]
    fn own_means_the_same_directory_whatever_the_spelling() {
        for image in [
            r"C:\Users\Ada\AppData\Local\Programs\Iamb\iamb-desktop.exe",
            r"c:\users\ada\appdata\local\programs\iamb\IAMB-DESKTOP.EXE",
            r"\\?\C:\Users\Ada\AppData\Local\Programs\Iamb\iamb-desktop.exe",
            "C:/Users/Ada/AppData/Local/Programs/Iamb/iamb-desktop.exe",
            SERVICE,
        ] {
            let identity = identify(Some(image), 3, Some(SERVICE));
            assert!(identity.own, "{image}");
            assert_eq!(identity.pid, 3);
        }
    }

    #[test]
    fn another_directory_is_another_application() {
        for image in [
            r"C:\Windows\System32\notepad.exe",
            // A directory below, a directory above and a name that only starts alike.
            r"C:\Users\Ada\AppData\Local\Programs\Iamb\plugins\iamb-desktop.exe",
            r"C:\Users\Ada\AppData\Local\Programs\iamb-desktop.exe",
            r"C:\Users\Ada\AppData\Local\Programs\Iamb2\iamb-desktop.exe",
            // No directory at all.
            "iamb-desktop.exe",
        ] {
            assert!(!identify(Some(image), 3, Some(SERVICE)).own, "{image}");
        }
        // Without the running executable's path nothing is Iamb's own.
        assert!(!identify(Some(SERVICE), 3, None).own);
        assert!(!same_directory("a.exe", "b.exe"));
    }

    #[test]
    fn the_identity_holds_no_directory() {
        let identity = identify(Some(SERVICE), 3, None);
        assert_eq!(identity.app.0, "iamb-service.exe");
        assert!(!format!("{identity:?}").contains("Ada"));
    }
}
