//! Prints the first rows of a repository's graph as text, for eyeballing
//! the layout: `cargo run --example dump -- <repo> [rows]`.

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| ".".into());
    let rows: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(40);
    let t = std::time::Instant::now();
    let repo = gr::Repo::open(&dir, &[]).expect("open repo");
    eprintln!("{} commits in {:?}", repo.commits.len(), t.elapsed());

    for (row, c) in repo.commits.iter().enumerate().take(rows) {
        // one char per lane: node 'o', a vertical line '|', a line shifting
        // left '/' or right '\' in the bottom half
        let mut cells = vec![' '; c.width * 2];
        for s in &repo.segs(row) {
            if s.y0 == 0.5 {
                let x = (s.x0 + s.x1) as usize; // midpoint, in half-lanes
                cells[x] = match s.x1.partial_cmp(&s.x0).unwrap() {
                    std::cmp::Ordering::Less => '/',
                    std::cmp::Ordering::Greater => '\\',
                    _ if cells[x] == ' ' => '|',
                    _ => cells[x],
                };
            }
        }
        cells[c.lane * 2] = 'o';
        let refs: Vec<&str> = c.refs.iter().map(|r| r.name.as_str()).collect();
        let refs = if refs.is_empty() { String::new() } else { format!("[{}] ", refs.join(", ")) };
        println!("{} {}{}", cells.iter().collect::<String>(), refs, c.subject());
    }
}
