//! Validação dos comandos administrativos e normalização literal de regras.
use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="snake_case")]
pub enum RuleKind { Source, Term, Topic }
impl RuleKind {
    pub const fn as_str(self)->&'static str {match self{Self::Source=>"source",Self::Term=>"term",Self::Topic=>"topic"}}
}
#[derive(Debug,Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleChange {pub kind:RuleKind,pub value:String,pub blocked:bool}
impl RuleChange {
    pub fn validate(&self)->Result<(),PolicyError> {
        if !(1..=160).contains(&self.value.chars().count())||self.value.chars().any(char::is_control)||text(&self.value).is_empty(){return Err(PolicyError::Input);}Ok(())
    }
}
/// Acentos são preservados; grafias alternativas exigem regras explícitas.
pub fn text(input:&str)->String {
    input.nfc().flat_map(char::to_lowercase).map(|c|if c.is_alphanumeric(){c}else{' '}).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}
#[derive(Debug,Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionRequest {
    pub request_id:Uuid,
    pub expected_status:String,
    pub expected_container_id:String,
    pub media_id:String,
    pub attest_creation_mapping:bool,
}
impl ResolutionRequest {
    pub fn validate(&self)->Result<(),PolicyError> {
        if self.request_id.is_nil()||!matches!(self.expected_status.as_str(),"uncertain"|"publishing"|"published_unresolved")||!numeric(&self.expected_container_id)||!numeric(&self.media_id)||!self.attest_creation_mapping{return Err(PolicyError::Input);}Ok(())
    }
}
pub fn numeric(value:&str)->bool {!value.is_empty()&&value.len()<=64&&value.bytes().all(|c|c.is_ascii_digit())}
#[derive(Debug,Error)]
pub enum PolicyError {#[error("pedido administrativo inválido")]Input}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    #[test]
    fn normalization_and_rules_are_bounded()->Result<(),PolicyError> {
        assert_eq!(text(" CORREC\u{327}A\u{303}O: Rust! "),"correção rust");
        assert_ne!(text("ação"),text("acao"));
        for value in ["","***","a\nsegredo"]{assert!(RuleChange{kind:RuleKind::Term,value:value.into(),blocked:true}.validate().is_err());}
        RuleChange{kind:RuleKind::Source,value:"Rust Blog".into(),blocked:true}.validate()?;
        let valid=ResolutionRequest{request_id:Uuid::new_v4(),expected_status:"uncertain".into(),expected_container_id:"123".into(),media_id:"456".into(),attest_creation_mapping:true};
        valid.validate()?;
        for mutation in 0..5 {let mut r=valid.clone();match mutation{0=>r.request_id=Uuid::nil(),1=>r.expected_status="failed".into(),2=>r.expected_container_id="".into(),3=>r.media_id="x".repeat(65),_=>r.attest_creation_mapping=false};assert!(r.validate().is_err());}Ok(())
    }
    proptest!{
        #[test]
        fn text_is_idempotent(value in "[a-zA-Z0-9 ,.!_-]{0,160}"){prop_assert_eq!(text(&text(&value)),text(&value));}
        #[test]
        fn media_identifier_cannot_be_a_command(value in "[a-zA-Z_-]{1,70}"){prop_assert!(!numeric(&value));}
    }
}
