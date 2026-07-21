#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermitKind {
    Request,
    Body,
    Planning,
    Database,
    Upstream,
    Stream,
}

#[derive(Debug)]
pub struct RequestPermitSet<Permit> {
    pub request: Permit,
    pub body: Option<Permit>,
    pub planning: Option<Permit>,
    pub db: Option<Permit>,
    pub upstream: Option<Permit>,
    pub stream: Option<Permit>,
}

impl<Permit> RequestPermitSet<Permit> {
    pub fn new(request: Permit) -> Self {
        Self {
            request,
            body: None,
            planning: None,
            db: None,
            upstream: None,
            stream: None,
        }
    }
}
