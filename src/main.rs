mod document;

use anyhow::{Context, Result, ensure};
use document::Document;
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, BufWriter},
    path::PathBuf,
    rc::{Rc, Weak},
};
use viewer_protocol::{API_VERSION, Command, PageTile, Request, Response, read_frame, write_frame};

#[derive(Default)]
struct Reader {
    hello: bool,
    views: HashMap<u64, Rc<Document>>,
    documents: HashMap<PathBuf, Weak<Document>>,
}

impl Reader {
    fn respond(&mut self, request: Request, response: &mut Response) -> Result<Vec<Vec<u8>>> {
        if let Command::Hello { api_version } = request.command {
            ensure!(
                api_version == API_VERSION,
                "Incompatible graphic extension API"
            );
            self.hello = true;
            return Ok(Vec::new());
        }
        ensure!(self.hello, "Handshake required");
        match request.command {
            Command::OpenPagedDocument { path } => {
                let path = std::fs::canonicalize(path).context("Could not open PDF file")?;
                ensure!(path.is_file(), "PDF path is not a regular file");
                let document = match self.documents.get(&path).and_then(Weak::upgrade) {
                    Some(document) => document,
                    None => Rc::new(Document::open(&path)?),
                };
                response.document = Some(document.metadata().clone());
                self.documents.insert(path, Rc::downgrade(&document));
                self.views.insert(request.view, document);
                self.documents
                    .retain(|_, document| document.strong_count() != 0);
                Ok(Vec::new())
            }
            Command::RenderPageTile { tile } => {
                let document = self
                    .views
                    .get(&request.view)
                    .context("Document view is closed")?;
                let pixels = document.render(tile)?;
                response.tile = Some(PageTile {
                    request: tile,
                    blob: 0,
                });
                Ok(vec![pixels])
            }
            Command::Close => {
                self.views.remove(&request.view);
                self.documents
                    .retain(|_, document| document.strong_count() != 0);
                Ok(Vec::new())
            }
            _ => anyhow::bail!("PDF Reader supports paged documents only"),
        }
    }
}

fn main() -> Result<()> {
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(io::stdout().lock());
    let mut reader = Reader::default();
    while !input.fill_buf()?.is_empty() {
        let frame = read_frame(&mut input).context("Invalid protocol frame")?;
        let request = serde_json::from_slice::<Request>(&frame);
        let mut response = Response::default();
        let result = match request {
            Ok(request) => {
                response.id = request.id;
                reader.respond(request, &mut response)
            }
            Err(error) => {
                response.id = serde_json::from_slice::<serde_json::Value>(&frame)
                    .ok()
                    .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))
                    .unwrap_or(0);
                Err(error).context("Invalid protocol request")
            }
        };
        let blobs = match result {
            Ok(blobs) => blobs,
            Err(error) => {
                response.error = Some(format!("{error:#}"));
                Vec::new()
            }
        };
        response.blobs = blobs.len();
        write_frame(&mut output, &serde_json::to_vec(&response)?)?;
        for blob in blobs {
            write_frame(&mut output, &blob)?;
        }
    }
    Ok(())
}
