//! Whether the SQLite3 ODBC driver KiCad reads the database through is
//! installed. Windows lists installed ODBC drivers in the registry; only
//! read here.

/// The driver's name, as the `.kicad_dbl` connection string asks for it.
pub const DRIVER: &str = "SQLite3 ODBC Driver";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Driver {
    Installed,
    Missing,
    /// Not checked on this system.
    Unknown,
}

/// Whether Windows lists the SQLite3 ODBC driver as installed.
#[cfg(windows)]
pub fn sqlite_driver() -> Driver {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
    let key = wide(r"SOFTWARE\ODBC\ODBCINST.INI\ODBC Drivers");
    let value = wide(DRIVER);
    let mut buffer = [0u16; 64];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: the key and value names end in 0, and `size` is the size of
    // `buffer` in bytes, which RegGetValueW writes no further than.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return Driver::Missing;
    }
    let length = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    if String::from_utf16_lossy(&buffer[..length]).eq_ignore_ascii_case("Installed") {
        Driver::Installed
    } else {
        Driver::Missing
    }
}

/// Other systems install ODBC drivers in their own ways; not checked.
#[cfg(not(windows))]
pub fn sqlite_driver() -> Driver {
    Driver::Unknown
}
