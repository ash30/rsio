#[derive(Debug, Copy, Clone)]
pub (crate) enum TransportError { 
    Generic
}

#[derive(Debug,Clone,Copy)]
pub enum TransportKind {
    Poll,
    Continuous
}

