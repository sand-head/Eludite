//! The Test Explorer corpus's Rust tests (brief 0035): `cargo test` lists and runs them, one fails, one is ignored, one
//! writes output, one waits when `CORPUS_SLOW_MS` is set (the cancel tests).

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn subtract(a: i32, b: i32) -> i32 {
    a - b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        let sum = add(2, 3);
        assert_eq!(sum, 5);
    }

    #[test]
    fn subtracts() {
        let difference = subtract(2, 3);
        assert_eq!(difference, 1);
    }

    #[test]
    #[ignore = "division is not written yet"]
    fn divides() {
        assert_eq!(4 / 2, 2);
    }

    #[test]
    fn writes_output() {
        println!("Hello from Rust");
    }

    #[test]
    fn waits() {
        if let Some(ms) = std::env::var("CORPUS_SLOW_MS").ok().and_then(|v| v.parse().ok()) {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }

    mod nested {
        use super::super::add;

        #[test]
        fn adds_negatives() {
            assert_eq!(add(-2, -3), -5);
        }
    }
}
