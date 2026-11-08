use std::path::{Path, PathBuf};

use crate::settings_surface::expand_user_path;

#[test]
fn tilde_paths_expand_and_relative_is_rejected() {
    let home = Path::new("/home/user");
    assert_eq!(
        expand_user_path("~/Backups/keyring.key", Some(home)).expect("tilde"),
        PathBuf::from("/home/user/Backups/keyring.key")
    );
    assert_eq!(
        expand_user_path("/tmp/keyring.key", None).expect("absolute"),
        PathBuf::from("/tmp/keyring.key")
    );
    assert!(expand_user_path("relative/keyring.key", Some(home)).is_err());
    assert!(expand_user_path("~other/keyring.key", Some(home)).is_err());
}
