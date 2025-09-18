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
/// Backup repositories. Contains the Github Client implementation for Backups
pub mod backup;
/// Manage branches for repositories. Contains the Github Client implementation for Branch management
pub mod branch;
/// Run various checks and sanity checks for a repository. Contains the Github Client
/// implementation for the various repository checks.
pub mod check;
/// The top level definition for the Github client
pub mod client;
/// Manages forked repositories, particularly updating the fork to match the upstream repository.
/// Contains the Github Client implementation for fork management.
pub mod fork;
/// Handle passed arguments
pub mod match_args;
/// Used to manage pull requests for libraries. Contains the Github Client implementation for
/// managing PRs
pub mod pr;
/// Manage releases. Contains the Github Client implementation for managing releases
pub mod release;
/// Manage tags. Contains the Github Client implementation for managing tags
pub mod tag;
