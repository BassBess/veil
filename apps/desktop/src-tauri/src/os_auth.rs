// Native verification is an application lock. Encryption keys remain account-bound
// in DPAPI / Keychain; the app never receives the user's OS password or biometrics.
pub async fn verify(window: tauri::Window) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || verify_native(&window))
        .await
        .map_err(|_| "System sign-in could not finish. The Vault remains locked.")?
}

#[cfg(windows)]
fn verify_native(window: &tauri::Window) -> Result<(), String> {
    use windows::{
        Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier},
        Win32::{Foundation::HWND, System::WinRT::IUserConsentVerifierInterop},
        core::{HSTRING, factory},
    };
    let _runtime = WindowsRuntime::new()?;
    check_windows_availability()?;
    let handle = window
        .hwnd()
        .map_err(|_| "Unable to attach Windows sign-in to the Vault")?;
    let interop: IUserConsentVerifierInterop =
        factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
            .map_err(|_| "Windows Hello is unavailable on this version of Windows")?;
    let operation: windows_future::IAsyncOperation<UserConsentVerificationResult> = unsafe {
        interop.RequestVerificationForWindowAsync(
            HWND(handle.0),
            &HSTRING::from("Verify your identity to access the VEIL Vault"),
        )
    }
    .map_err(|_| "Windows could not show its sign-in prompt. The Vault remains locked.")?;
    let result = operation
        .join()
        .map_err(|_| "Windows sign-in failed. The Vault remains locked.")?;
    windows_result(result)
}

#[cfg(windows)]
struct WindowsRuntime;
#[cfg(windows)]
impl WindowsRuntime {
    fn new() -> Result<Self, String> {
        use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|_| "Unable to initialize Windows sign-in")?;
        Ok(Self)
    }
}
#[cfg(windows)]
impl Drop for WindowsRuntime {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::WinRT::RoUninitialize() };
    }
}

#[cfg(windows)]
fn check_windows_availability() -> Result<(), String> {
    use windows::Security::Credentials::UI::{
        UserConsentVerifier, UserConsentVerifierAvailability as A,
    };
    let availability = UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|operation| operation.join())
        .map_err(|_| "Unable to check Windows Hello. Choose a Vault passphrase instead.")?;
    match availability {
        A::Available => Ok(()),
        A::NotConfiguredForUser => Err("Set up a Windows Hello PIN in Windows Settings > Accounts > Sign-in options, or choose a Vault passphrase.".into()),
        A::DisabledByPolicy => Err("Windows Hello is disabled by your administrator. Choose a Vault passphrase.".into()),
        A::DeviceBusy => Err("Windows Hello is busy. Try again.".into()),
        _ => Err("Windows Hello is unavailable. Set up Windows sign-in or choose a Vault passphrase.".into()),
    }
}

#[cfg(windows)]
fn windows_result(
    result: windows::Security::Credentials::UI::UserConsentVerificationResult,
) -> Result<(), String> {
    use windows::Security::Credentials::UI::UserConsentVerificationResult as R;
    match result {
        R::Verified => Ok(()),
        R::Canceled => Err("Sign-in was cancelled. The Vault remains locked.".into()),
        R::RetriesExhausted => Err("Too many sign-in attempts. The Vault remains locked.".into()),
        _ => Err("Identity verification did not succeed. The Vault remains locked.".into()),
    }
}

#[cfg(target_os = "macos")]
fn verify_native(_window: &tauri::Window) -> Result<(), String> {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};
    let context = unsafe { LAContext::new() };
    let policy = LAPolicy::DeviceOwnerAuthentication;
    unsafe { context.canEvaluatePolicy_error(policy) }
        .map_err(|_| "macOS sign-in is unavailable. Choose a Vault passphrase instead.")?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    // Only a bool crosses the callback thread; no Objective-C objects are shared.
    let reply = RcBlock::new(move |success: Bool, _error: *mut NSError| {
        let _ = sender.send(success.as_bool());
    });
    let reason = NSString::from_str("access your VEIL Vault");
    unsafe { context.evaluatePolicy_localizedReason_reply(policy, &reason, &reply) };
    let verified = receiver
        .recv_timeout(std::time::Duration::from_secs(120))
        .unwrap_or(false);
    unsafe { context.invalidate() };
    if verified {
        Ok(())
    } else {
        Err("macOS sign-in was cancelled or failed. The Vault remains locked.".into())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn verify_native(_window: &tauri::Window) -> Result<(), String> {
    Err("System sign-in is supported on Windows and macOS. Choose a Vault passphrase.".into())
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn verification_fails_closed_for_every_non_success_result() {
        use windows::Security::Credentials::UI::UserConsentVerificationResult as R;
        assert!(super::windows_result(R::Verified).is_ok());
        for result in [
            R::Canceled,
            R::RetriesExhausted,
            R::DeviceBusy,
            R::DeviceNotPresent,
            R::NotConfiguredForUser,
            R::DisabledByPolicy,
        ] {
            assert!(super::windows_result(result).is_err());
        }
    }
}
