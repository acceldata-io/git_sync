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

use serde::Deserialize;

#[derive(Deserialize)]
pub struct RepoResponse {
    pub data: RepoData,
}
#[derive(Deserialize)]
pub struct RepoData {
    pub repository: Repository,
}
#[derive(Deserialize)]
pub struct Repository {
    pub parent: Option<ParentRepo>,
    pub refs: Refs,
}
#[derive(Deserialize)]
pub struct ParentRepo {
    pub url: String,
}
#[derive(Deserialize)]
pub struct Refs {
    pub nodes: Vec<TagNode>,
    #[serde(rename = "pageInfo")]
    pub page_info: PageInfo,
}
#[derive(Deserialize)]
pub struct PageInfo {
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
    #[serde(rename = "endCursor")]
    pub end_cursor: Option<String>,
}
#[derive(Deserialize)]
pub struct TagNode {
    pub name: String,
    pub target: TagTarget,
}
#[derive(Deserialize)]
pub struct TagTarget {
    #[serde(rename = "__typename")]
    pub typename: String,
    pub oid: String,
    pub message: Option<String>,
    pub tagger: Option<Tagger>,
}
#[derive(Deserialize)]
pub struct Tagger {
    pub name: Option<String>,
    pub email: Option<String>,
    pub date: Option<String>,
}
