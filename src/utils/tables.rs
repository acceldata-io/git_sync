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
use console::{Term, measure_text_width};
use std::fmt;
use tabled::{
    builder::Builder,
    settings::{Alignment, Panel, style::Style},
};

/// A struct wrapping around tabled's tables so that we can build tables more easily
pub struct Table<T, B, L, R, H, V, const HSIZE: usize, const VSIZE: usize> {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    title: Option<String>,
    style: Style<T, B, L, R, H, V, HSIZE, VSIZE>,
    alignment: Option<Alignment>,
    centre: bool,
    table: tabled::Table,
}

impl<
    T: Clone,
    B: Clone,
    L: Clone,
    R: Clone,
    H: Clone,
    V: Clone,
    const HSIZE: usize,
    const VSIZE: usize,
> Table<T, B, L, R, H, V, HSIZE, VSIZE>
{
    pub fn builder(style: Style<T, B, L, R, H, V, HSIZE, VSIZE>) -> Self {
        Table {
            header: Vec::new(),
            rows: Vec::new(),
            title: None,
            style,
            alignment: None,
            centre: false,
            table: tabled::Table::default(),
        }
    }
    /// Add a header to the table
    pub fn header<I, S>(mut self, header: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.header = header.into_iter().map(Into::into).collect();
        self
    }

    /// Set the rows for the table
    pub fn rows<I, S>(mut self, rows: I) -> Self
    where
        I: IntoIterator,
        I::Item: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rows = rows
            .into_iter()
            .map(|row| row.into_iter().map(Into::into).collect())
            .collect();

        self
    }
    /// Add a title to your table
    pub fn title<S: Into<String>>(mut self, title: S) -> Self {
        self.title = Some(title.into());
        self
    }
    /// Set the alignment of the row/column
    pub fn align(mut self, alignment: Alignment) -> Self {
        self.alignment = Some(alignment);
        self
    }
    /// Define if the table should be centred in the terminal
    pub fn centre(mut self, centre: bool) -> Self {
        self.centre = centre;
        self
    }
    /// Build the table
    pub fn build(mut self) -> Self {
        // Use tabled's builder pattern
        let mut builder = Builder::default();
        if !self.header.is_empty() {
            builder.push_record(&self.header);
        }
        for row in &self.rows {
            builder.push_record(row);
        }
        let mut table = builder.build();
        table.with(self.style.clone());

        if let Some(ref title) = self.title {
            table.with(Panel::horizontal(0, title));
        }

        if let Some(alignment) = self.alignment {
            table.with(alignment);
        }
        self.table = table;

        self
    }
}

impl<
    T: fmt::Debug,
    B: fmt::Debug,
    L: fmt::Debug,
    R: fmt::Debug,
    H: fmt::Debug,
    V: fmt::Debug,
    const HSIZE: usize,
    const VSIZE: usize,
> fmt::Display for Table<T, B, L, R, H, V, HSIZE, VSIZE>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let term = Term::stdout();

        let table = if self.rows.is_empty() {
            "".to_string()
        } else if self.centre && term.is_term() {
            let term_width = term.size().1 as usize;
            self.table
                .to_string()
                .lines()
                .map(|line| {
                    let line_width = measure_text_width(line);
                    let padding = if term_width > line_width {
                        (term_width - line_width) / 2
                    } else {
                        0
                    };
                    let padding_amount = " ".repeat(padding);
                    format!("{padding_amount}{line}{padding_amount}")
                })
                .collect::<Vec<String>>()
                .join("\n")
        } else if term.is_term() {
            self.table.to_string()
        } else {
            let mut output = String::new();
            for row in &self.rows {
                let line = row.join(",");
                output.push_str(&line);
                output.push('\n');
            }
            output
        };

        write!(f, "{table}")
    }
}
