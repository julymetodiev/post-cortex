# Contributing to Post-Cortex

We appreciate your interest in contributing to Post-Cortex! This document provides guidelines and information to help you contribute effectively to our lock-free conversation memory system.

## Getting Started

### Development Environment Setup

1. **Prerequisites**
   - Rust 1.70+ with edition 2024 support
   - Git for version control
   - Preferred IDE with Rust support (VS Code with rust-analyzer, IntelliJ-Rust, etc.)

2. **Repository Setup**
   ```bash
   git clone https://github.com/julymetodiev/post-cortex.git
   cd post-cortex
   cargo build --features embeddings
   ```

3. **Running Tests**
   ```bash
   # Run all tests
   cargo test --features embeddings
   
   # Run with specific output
   cargo test --features embeddings -- --nocapture
   ```

4. **Building Documentation**
   ```bash
   cargo doc --no-deps --open
   ```

## Understanding the Architecture

Before contributing, it's important to understand the core architectural principles of Post-Cortex:

### Lock-Free Design
Post-Cortex is built with a strict lock-free architecture to ensure zero deadlocks and high concurrency:

- We use `DashMap` instead of `RwLock<HashMap>`
- We use `ArcSwap` for atomic pointer swaps
- Atomic operations (`AtomicU64`, `AtomicUsize`, `AtomicBool`) for shared state
- Actor patterns for complex operations

### Memory Hierarchy
The system uses a three-tier memory architecture:
- Hot Memory (50 items): Frequently accessed, instant access
- Warm Memory (200 items): Less frequent access, compressed cache
- Cold Storage: RocksDB for persistent storage

## Contribution Guidelines

### Code Quality Standards

1. **Maintain Lock-Free Architecture**
   - Never introduce `Mutex` or `RwLock` without explicit justification
   - Use appropriate concurrent data structures (`DashMap`, `ArcSwap`)
   - Follow the actor pattern for complex operations
   - Verify your changes don't introduce potential deadlocks

2. **Testing Requirements**
   - Add comprehensive tests for new functionality
   - Include concurrency tests when relevant
   - Ensure all tests pass before submitting PR
   - Aim for high code coverage in critical paths

3. **Documentation**
   - Update public API documentation for new features
   - Include examples for complex functionality
   - Document architectural decisions in commit messages
   - Add inline comments for non-obvious code sections

4. **Code Style**
   - Follow Rust idioms and conventions
   - Use `cargo fmt` to format code
   - Use `cargo clippy` to catch common issues
   - Write clear, descriptive variable and function names

### Contribution Workflow

1. **Fork the Repository**
   - Create a personal fork of the Post-Cortex repository
   - Clone your fork locally

2. **Create a Feature Branch**
   ```bash
   git checkout -b feature/your-feature-name
   ```

3. **Make Your Changes**
   - Implement your feature or bug fix
   - Ensure all tests pass
   - Follow the code style guidelines
   - Update documentation as needed

4. **Commit Your Changes**
   - Follow our Git commit message guidelines
   - Make atomic, logical commits
   - Include context for complex changes

5. **Push to Your Fork**
   ```bash
   git push origin feature/your-feature-name
   ```

6. **Create a Pull Request**
   - Provide a clear description of your changes
   - Reference relevant issues
   - Include testing instructions if needed
   - Ensure continuous integration passes

## Types of Contributions

### Bug Fixes
- Clearly describe the bug being fixed
- Include steps to reproduce the issue
- Add tests that would fail without the fix but pass with it

### Features
- Explain the use case and motivation
- Document the design approach
- Include comprehensive tests
- Update relevant documentation

### Documentation
- Fix typos, grammatical errors, or unclear explanations
- Add missing documentation for existing features
- Improve examples and tutorials
- Ensure API documentation is complete and accurate

### Performance Improvements
- Benchmark before and after changes
- Document performance improvements with data
- Explain any trade-offs made
- Ensure improvements don't compromise correctness

## Code Review Process

### What Reviewers Look For
- Adherence to lock-free architecture principles
- Code quality and readability
- Test coverage and quality
- Documentation completeness
- Performance implications

### Review Guidelines
- Be constructive and respectful in feedback
- Focus on the code, not the author
- Explain the reasoning behind suggested changes
- Acknowledge good practices and solutions

## Testing Strategy

### Unit Tests
- Test individual functions and methods in isolation
- Cover edge cases and error conditions
- Use property-based testing for appropriate functions

### Integration Tests
- Test interaction between components
- Verify end-to-end functionality
- Include concurrency tests where relevant

### Benchmark Tests
- Use Criterion for performance-sensitive code
- Include before/after measurements for optimizations
- Document benchmark results in PRs

## Security Considerations

- Follow secure coding practices
- Validate all external inputs
- Be cautious with unsafe code
- Consider potential side-channel attacks

## Community Resources

- **Discussions**: Use GitHub Discussions for questions and ideas
- **Issues**: Report bugs and request features via GitHub Issues
- **Documentation**: Contribute to improving documentation

## Code of Conduct

We are committed to providing a friendly, safe, and welcoming environment for all participants. Please:

- Be respectful and inclusive
- Welcome newcomers and help them learn
- Focus on what is best for the community
- Show empathy towards other community members

## Getting Help

If you need help with your contribution:

- Check existing documentation and issues
- Ask questions in GitHub Discussions
- Reach out to maintainers for guidance

Thank you for contributing to Post-Cortex!