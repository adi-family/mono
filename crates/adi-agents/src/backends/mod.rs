pub(crate) mod process;
pub(crate) mod tmux;

pub(crate) fn push_option(argv: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        argv.extend([flag.into(), value.into()]);
    }
}
