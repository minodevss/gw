use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub(in crate::provider) struct Reply {
    pub(in crate::provider) status: u16,
    pub(in crate::provider) body: String,
}

pub(in crate::provider) struct FakeServer {
    pub(in crate::provider) base_url: String,
    requests: JoinHandle<Vec<String>>,
}

impl FakeServer {
    pub(in crate::provider) async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = tokio::spawn(async move {
            let mut seen = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                seen.push(read_request(&mut stream).await);
                let response = format!(
                    "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    reply.status,
                    reply.body.len(),
                    reply.body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
            seen
        });
        Self { base_url, requests }
    }

    pub(in crate::provider) async fn requests(self) -> Vec<String> {
        self.requests.await.unwrap()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.unwrap();
        data.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&data).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if body.len() >= length {
                return text;
            }
        }
        if read == 0 {
            return text;
        }
    }
}
