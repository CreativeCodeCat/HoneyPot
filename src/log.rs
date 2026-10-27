//! Simple colored terminal logging, mirroring the old `colors.py`.

const RESET: &str = "\x1b[0m";

fn print(color: &str, message: impl AsRef<str>) {
    println!("{color}{}{RESET}", message.as_ref());
}

pub fn info(message: impl AsRef<str>) {
    print("\x1b[92m", message);
}

pub fn error(message: impl AsRef<str>) {
    print("\x1b[91m", message);
}

pub fn warning(message: impl AsRef<str>) {
    print("\x1b[93m", message);
}

pub fn debug(message: impl AsRef<str>) {
    print("\x1b[96m", message);
}

pub fn blue(message: impl AsRef<str>) {
    print("\x1b[94m", message);
}
