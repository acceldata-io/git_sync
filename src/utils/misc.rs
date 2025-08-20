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

/// Convert github http status errors to a usable string message
pub fn get_http_status(err: &octocrab::Error) -> (Option<http::StatusCode>, Option<String>) {
    if let octocrab::Error::GitHub { source, .. } = err {
        let status = source.status_code;
        let message = source.message.clone();
        return (Some(status), Some(message));
    }
    (None, None)
}
