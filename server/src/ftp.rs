use unftp_sbe_fs::ServerExt;

pub async fn start_ftp_server() {
    let addr = "0.0.0.0:2121";
    let server = libunftp::Server::with_fs(std::env::temp_dir())
        .greeting("Welcome to Speedupdate FTP server");

    tracing::info!("FTP server listening on {}", addr);
    server.listen(addr).await.unwrap();
}
