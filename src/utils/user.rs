use std::process::Command;

/// Hold information about the git user
#[derive(Debug)]
pub struct UserDetails {
    /// Name of the user
    pub name: String,
    /// Email of the user
    pub email: String,
}

impl UserDetails {
    /// Create a new UserDetails instance
    pub fn new(name: String, email: String) -> Self {
        UserDetails { name, email }
    }
    /// Try to load user information from the environment through git
    pub fn new_from_git() -> Result<Self, std::io::Error> {
        let name_output = Command::new("git").arg("config").arg("get").arg("user.name").output()?;
        let email_output = Command::new("git").arg("config").arg("get").arg("user.email").output()?;

        match (name_output.status.success(), email_output.status.success()) {
            (true, true) => {
                let name = String::from_utf8_lossy(&name_output.stdout).trim().to_string();
                let email = String::from_utf8_lossy(&email_output.stdout).trim().to_string();
                Ok(UserDetails { name, email })
            }
            (false, _) => Err(std::io::Error::other("Failed to get git user name")),
            (_, false) => Err(std::io::Error::other("Failed to get git email address")),
        }
    }
}

