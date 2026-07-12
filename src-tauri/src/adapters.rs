#![allow(dead_code)]

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppInstallation {
    pub executable: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AccountIdentity {
    pub external_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SnapshotScope {
    pub include_user_data: bool,
}

pub trait AppAdapter {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn discover(&self) -> Result<AppInstallation, String>;
    fn inspect_current_account(
        &self,
        installation: &AppInstallation,
    ) -> Result<Option<AccountIdentity>, String>;
    fn scan_accounts(&self, installation: &AppInstallation)
        -> Result<Vec<AccountIdentity>, String>;
    fn stop(&self, installation: &AppInstallation) -> Result<(), String>;
    fn start(&self, installation: &AppInstallation) -> Result<(), String>;
    fn is_running(&self, installation: &AppInstallation) -> bool;
}

pub struct OopzAdapter;

impl OopzAdapter {
    pub const ID: &'static str = "oopz";
    pub const DISPLAY_NAME: &'static str = "OOPZ";
}
