//! Thème sombre pour les menus natifs Windows (barre + popups « Fichier », etc.).

#[cfg(windows)]
pub fn force_dark_popups() {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

    #[repr(C)]
    #[allow(dead_code)]
    enum PreferredAppMode {
        Default,
        AllowDark,
        ForceDark,
    }

    type SetPreferredAppMode = unsafe extern "system" fn(PreferredAppMode) -> PreferredAppMode;
    type AllowDarkModeForApp = unsafe extern "system" fn(bool) -> bool;
    type FlushMenuThemes = unsafe extern "system" fn();

    const UXTHEME_SETPREFERREDAPPMODE: usize = 135;
    const UXTHEME_ALLOWDARKMODEFORAPP: usize = 135;
    const UXTHEME_FLUSHMENUTHEMES: usize = 136;

    unsafe {
        let uxtheme = LoadLibraryA(b"uxtheme.dll\0".as_ptr());
        if uxtheme.is_null() {
            return;
        }

        let set_preferred = GetProcAddress(uxtheme, UXTHEME_SETPREFERREDAPPMODE as *const u8);
        if let Some(proc) = set_preferred {
            let set_preferred: SetPreferredAppMode = std::mem::transmute(proc);
            set_preferred(PreferredAppMode::ForceDark);
        } else if let Some(proc) = GetProcAddress(uxtheme, UXTHEME_ALLOWDARKMODEFORAPP as *const u8)
        {
            let allow_dark: AllowDarkModeForApp = std::mem::transmute(proc);
            allow_dark(true);
        }

        if let Some(proc) = GetProcAddress(uxtheme, UXTHEME_FLUSHMENUTHEMES as *const u8) {
            let flush: FlushMenuThemes = std::mem::transmute(proc);
            flush();
        }

        // Rafraîchit l'état des couleurs immersives (comme tao).
        const UXTHEME_REFRESHIMMERSIVECOLORPOLICYSTATE: usize = 104;
        if let Some(proc) = GetProcAddress(
            uxtheme,
            UXTHEME_REFRESHIMMERSIVECOLORPOLICYSTATE as *const u8,
        ) {
            type RefreshImmersiveColorPolicyState = unsafe extern "system" fn();
            let refresh: RefreshImmersiveColorPolicyState = std::mem::transmute(proc);
            refresh();
        }
    }
}

#[cfg(not(windows))]
pub fn force_dark_popups() {}
