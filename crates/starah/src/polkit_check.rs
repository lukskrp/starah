//! starah-polkit-check ACTION USER [DETAIL]
//!
//! Run by polkitd (through `polkit.spawn` in Starah's rule) for every authorization
//! question about an agent user. Prints `yes` or `no`. Any failure prints `no`: when
//! starahd cannot answer, agents get nothing.

mod proxy;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (action, user, detail) = match args.as_slice() {
        [a, u] => (a.as_str(), u.as_str(), ""),
        [a, u, d] => (a.as_str(), u.as_str(), d.as_str()),
        _ => {
            eprintln!("usage: starah-polkit-check ACTION USER [DETAIL]");
            println!("no");
            return;
        }
    };
    let answer = async {
        let conn = zbus::Connection::system().await?;
        let broker = proxy::Broker1Proxy::new(&conn).await?;
        broker.check(action, user, detail).await
    };
    match tokio::time::timeout(std::time::Duration::from_secs(8), answer).await {
        Ok(Ok((true, _))) => println!("yes"),
        Ok(Ok((false, why))) => {
            eprintln!("starah: {user} {action}: {why}");
            println!("no");
        }
        Ok(Err(e)) => {
            eprintln!("starah: cannot ask starahd: {e}");
            println!("no");
        }
        Err(_) => {
            eprintln!("starah: starahd did not answer in time");
            println!("no");
        }
    }
}
