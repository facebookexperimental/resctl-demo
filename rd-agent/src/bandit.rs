use rd_agent_intf::Bandit;

mod mem_hog;

pub fn bandit_main(bandit: &Bandit) {
    match bandit {
        Bandit::MemHog(args) => mem_hog::bandit_mem_hog(args),
    }
}
