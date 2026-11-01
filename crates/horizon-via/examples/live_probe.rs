fn main() {
    let r = horizon_via::probe::probe_server("127.0.0.1:25599", 5);
    match r {
        Ok(v) => println!("OK: {} protocol {} motd {:?}", v.version_name, v.protocol, v.motd),
        Err(e) => println!("ERR: {}", e),
    }
}