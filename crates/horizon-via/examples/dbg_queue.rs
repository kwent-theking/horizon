fn main() {
    use horizon_via::frame::FrameQueue;
    let mut payload = vec![0x8F, 0x10, 0x78, 0x9C];
    payload.extend(std::iter::repeat(0xAB).take(812));
    let mut q = FrameQueue::new();
    q.push(&[0xB0, 0x06]);
    q.push(&payload);
    match q.pop_raw() {
        Some(Ok(raw)) => println!("first OK {}", raw.len()),
        other => println!("first {:?}", other.map(|r| r.map(|v| v.len()))),
    }
    q.push(&[0x1C]);
    let zeros = vec![0x00u8; 28];
    q.push(&zeros);
    println!("pending before 2nd: {}", q.pending());
    match q.pop_raw() {
        Some(Ok(raw)) => println!("second OK {}", raw.len()),
        other => println!("second {:?}", other.map(|r| r.map(|v| v.len()))),
    }
}
