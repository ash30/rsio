#[derive(Debug, Copy, Clone)]
pub enum TransportError { 
    Generic
}

#[derive(Debug,Clone,Copy)]
pub enum TransportKind {
    Poll,
    Continuous
}

