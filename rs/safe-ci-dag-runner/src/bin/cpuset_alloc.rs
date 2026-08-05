fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(safe_ci_dag_runner::cpuset_allocator::run(&args));
}
