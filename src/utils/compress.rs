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

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::fs::File;
use std::path::{Path, PathBuf};
use tar::Archive;
use tempdir::TempDir;

use crate::error::GitError;

/// Compress a directory
pub fn compress_directory(path: &Path) -> Result<PathBuf, GitError> {
    if !path.exists() {
        return Err(GitError::FileDoesNotExist(
            path.to_string_lossy().to_string(),
        ));
    }
    let string_path = path.as_os_str().to_string_lossy().to_string();
    let tarball_path = format!("{string_path}.tar.gz");
    if PathBuf::from(&tarball_path).exists() {
        std::fs::remove_file(&tarball_path)?;
    }
    let tar_gz = File::create(&tarball_path)?;
    let buffer = std::io::BufWriter::new(tar_gz);

    let encoder = GzEncoder::new(buffer, Compression::fast());

    let mut tar = tar::Builder::new(encoder);
    let dir = path.file_name();
    if let Some(dir) = dir {
        tar.append_dir_all(dir, path)?;
        return Ok(PathBuf::from(tarball_path));
    }
    Err(GitError::Other(format!(
        "Could not compress {}",
        path.display()
    )))
}

/// Extract a tar.gz. This is only needed to test that the compression functions as expected
#[allow(unused)]
fn decode_tar_gz(path: &Path, dest: &Path) -> Result<PathBuf, GitError> {
    if !path.exists() {
        return Err(GitError::FileDoesNotExist(
            path.to_string_lossy().to_string(),
        ));
    }

    let file = File::open(path)?;
    let input = std::io::BufReader::new(file);
    let gz = GzDecoder::new(input);

    let mut archive = Archive::new(gz);

    let tmp_dir = TempDir::new("")?;
    archive.unpack(tmp_dir.path())?;
    if !dest.exists() {
        std::fs::create_dir(dest)?;
    }

    for entry in std::fs::read_dir(tmp_dir.path())? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        std::fs::rename(from, to)?;
    }

    let entries = std::fs::read_dir(dest)?
        .filter_map(std::result::Result::ok)
        .collect::<Vec<_>>();

    if entries.is_empty() {
        return Err(GitError::Other("Extraction failed".to_string()));
    }
    for entry in &entries {
        println!("Entry: {entry:#?}");
    }
    let top_level = entries[0].path();
    Ok(top_level)
}

#[cfg(test)]
mod tests {
    use crate::utils::compress::{compress_directory, decode_tar_gz};
    use std::path::PathBuf;
    use std::{fs::File, io::Write};
    use tempdir::TempDir;

    #[test]
    fn test_compress_directory() {
        let tmp_dir = TempDir::new("").unwrap();
        let tmp = tmp_dir.path();
        let tmp_str = tmp.to_str().unwrap();

        let my_dir = format!("{tmp_str}/my_dir");

        std::fs::create_dir(&my_dir).unwrap();

        let mut file = File::create(format!("{my_dir}/my_file.txt")).unwrap();

        let _ = writeln!(file, "This is some text");
        let p = PathBuf::from(&my_dir);

        let path = compress_directory(&p).unwrap();

        let extracted =
            decode_tar_gz(&path, &PathBuf::from(format!("{tmp_str}/my_test_dir"))).unwrap();

        let file_path = extracted.join("my_file.txt");
        let contents = std::fs::read_to_string(file_path).unwrap();
        assert_eq!(contents.trim(), "This is some text");
    }
}
