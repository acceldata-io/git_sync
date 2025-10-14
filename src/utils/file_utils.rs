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
/// Recursively replace all occurrences of the regex `re` with `replacement` in all files in the
/// directory `path`
pub fn replace_all_in_directory<T: AsRef<str> + Copy>(
    path: &std::path::Path,
    re: &Regex,
    replacement: T,
) {
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && e.file_name() == ".git"))
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() {
            let file_path = entry.path();
            match replace_in_file(file_path, re, replacement) {
                Ok(true) => println!("Modified file: {}", file_path.display()),
                Ok(false) => (),
                Err(e) => {
                    eprintln!("Skipping file {}: {e}", file_path.display());
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
