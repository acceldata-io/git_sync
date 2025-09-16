#!/bin/sh

cargo doc --no-deps
rm -rf ./docs
echo "<meta http-equiv=\"refresh\" content=\"0; url=git_sync\">" > target/doc/index.html
cp -r target/doc ./docs
