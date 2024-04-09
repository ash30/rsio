#[derive(Debug, Clone)]
pub (crate) enum TransportError { 
}

#[derive(Debug,Clone,Copy)]
pub enum TransportKind {
    Poll,
    Continuous
}

