/*
Licensed to the Apache Software Foundation (ASF) under one
or more contributor license agreements.  See the NOTICE file
distributed with this work for additional information
regarding copyright ownership.  The ASF licenses this file
to you under the Apache License, Version 2.0 (the
"License"); you may not use this file except in compliance
with the License.  You may obtain a copy of the License at

  http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing,
software distributed under the License is distributed on an
"AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
KIND, either express or implied.  See the License for the
specific language governing permissions and limitations
under the License.
*/

use fancy_regex::Captures;
use fancy_regex::Regex;
use std::fs;
use std::io;
use std::path::Path;
use walkdir::WalkDir;
/// Replace all occurrences of the regex `re` with `replacement`
pub fn replace_in_file<T: AsRef<str>>(
    path: &std::path::Path,
    re: &Regex,
    replacement: T,
) -> io::Result<bool> {
    let content = fs::read_to_string(path)?;
    let new_content = re.replace_all(&content, replacement.as_ref());
    if new_content == content {
        Ok(false)
    } else {
        fs::write(path, new_content.as_ref())?;
        Ok(true)
    }
}
/// Replace all occurrences of the regex `re` with the result of calling the `replacement` function
pub fn replace_in_file_with<F>(
    path: &std::path::Path,
    re: &Regex,
    replacement: &F,
) -> io::Result<bool>
where
    F: Fn(&Captures) -> String,
{
    let content = fs::read_to_string(path)?;
    let new_content = re.replace_all(&content, |captures: &Captures| replacement(captures));
    if new_content == content {
        Ok(false)
    } else {
        fs::write(path, new_content.as_ref())?;
        Ok(true)
    }
}

/// Recursively replace all occurrences of the regex `re` with `replacement` in all files in the
/// directory `path`
pub fn replace_all_in_directory<T: AsRef<str> + Copy>(
    path: &std::path::Path,
    re: &Regex,
    replacement: T,
    quiet: bool,
) {
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && e.file_name() == ".git"))
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() {
            let file_path = entry.path();
            match replace_in_file(file_path, re, replacement) {
                Ok(true) => {
                    if !quiet {
                        println!("Modified file: {}", file_path.display());
                    }
                }
                Ok(false) => (),
                Err(e) => {
                    if !quiet {
                        eprintln!("Skipping file {}: {e}", file_path.display());
                    }
                }
            }
        }
    }
}
/// Replace all occurrences of the regex `re` with the result of calling the `replacement` function
pub fn replace_all_in_directory_with<F>(
    path: &std::path::Path,
    re: &Regex,
    replacement: &F,
    quiet: bool,
) where
    F: Fn(&Captures) -> String,
{
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && e.file_name() == ".git"))
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() {
            let file_path = entry.path();
            match replace_in_file_with(file_path, re, replacement) {
                Ok(true) => {
                    if !quiet {
                        println!("Modified file: {}", file_path.display());
                    }
                }
                Ok(false) => (),
                Err(e) => {
                    if !quiet {
                        eprintln!("Skipping file {}: {e}", file_path.display());
                    }
                }
            }
        }
    }
}

/// Copy all files and directories from `src` to `dst` recursively
pub fn copy_recursive(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_recursive(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::filter::get_or_compile;
    use tempdir::TempDir;
    /// If using this function, make sure to hold onto the `TempDir` reference
    /// because once it goes out of scope, that temp directory is deleted.
    fn create_test_file(filename: &str, content: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new("test").unwrap();
        let file_path = dir.path().join(filename);
        fs::write(&file_path, content).unwrap();
        (dir, file_path)
    }
    #[test]
    fn replace_text_using_clojure() {
        let input = "ODP_BN=\"3001\"\nodp_bn=\"3001\"";
        // Hold onto the dir reference so that the temp dir isn't deleted
        let (_dir, file) = create_test_file("test.txt", input);

        let re = get_or_compile(r#"(?i)(odp_bn)(?-i)="\d+"#).unwrap();
        let replacement = |captures: &Captures| format!(r#"{}="{}"#, &captures[1], "3002");
        replace_in_file_with(&file, &re, &replacement).unwrap();
        let got = fs::read_to_string(&file).unwrap();
        assert!(got.contains(r#"ODP_BN="3002"#));
        assert!(got.contains(r#"odp_bn="3002"#));
        assert!(!got.contains("3001"));
    }
    #[test]
    fn replace_text_no_matches() {
        let input = "ODPA_BN=\"3001\"\nodpB_bn=\"3001\"";
        let (_dir, file) = create_test_file("test.txt", input);

        let re = get_or_compile(r#"(odp_bn|ODP_BN)="\d+"#).unwrap();
        let replacement = |captures: &Captures| format!(r#"{}="{}"#, &captures[1], "3002");
        replace_in_file_with(&file, &re, &replacement).unwrap();
        let got = fs::read_to_string(&file).unwrap();
        eprintln!("{got}");
        assert!(got.contains(r#"ODPA_BN="3001"#));
        assert!(got.contains(r#"odpB_bn="3001"#));
    }
    #[test]
    fn replace_all_in_directory_with_closure() {
        // create a temp directory and two files (one in a subdir) to ensure recursion works

        let (dir, file1) = create_test_file("a.txt", "ODP_BN=\"3001\"\nodp_bn=\"3001\"");
        let subdir = dir.path().join("sub");
        std::fs::create_dir_all(&subdir).unwrap();
        let file2_path = subdir.join("b.txt");
        fs::write(&file2_path, "ODP_BN=\"3001\"\nodp_bn=\"3001\"").unwrap();

        let re = get_or_compile(r#"(?i)(odp_bn)(?-i)="\d+"#).unwrap();

        let replacement = |captures: &Captures| format!(r#"{}="{}"#, &captures[1], "3002");

        replace_all_in_directory_with(dir.path(), &re, &replacement, false);

        let got1 = fs::read_to_string(&file1).unwrap();
        assert!(got1.contains(r#"ODP_BN="3002"#));
        assert!(got1.contains(r#"odp_bn="3002"#));
        assert!(!got1.contains("3001"));

        let got2 = fs::read_to_string(&file2_path).unwrap();
        assert!(got2.contains(r#"ODP_BN="3002"#));
        assert!(got2.contains(r#"odp_bn="3002"#));
        assert!(!got2.contains("3001"));
    }
}
