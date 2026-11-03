fn main() -> std::process::ExitCode {
    macscan::entry(std::env::args_os().skip(1).collect(), macscan_analysis::machine::plan)
}
