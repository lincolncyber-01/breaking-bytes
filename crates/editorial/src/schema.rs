use serde_json::{Value, json};

pub fn draft() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["title","summary","source","kind","entities","language"],"properties":{
        "title":{"type":"string","description":"Título ativo, neutro, até 8 palavras e 160 caracteres."},
        "summary":{"type":"array","items":{"type":"string"},"description":"De 2 a 4 linhas, até 100 caracteres cada."},
        "source":{"type":"string"},"kind":{"type":"string","enum":["new","update","correction"]},
        "entities":{"type":"array","items":{"type":"string"}},"language":{"type":"string","enum":["pt-BR"]}}})
}

pub fn check() -> Value {
    let citation = json!({"type":"object","additionalProperties":false,"required":["article_id","quote"],"properties":{"article_id":{"type":"integer"},"quote":{"type":"string"}}});
    let claim = json!({"type":"object","additionalProperties":false,"required":["text","supported","evidence"],"properties":{"text":{"type":"string"},"supported":{"type":"boolean"},"evidence":{"type":"array","items":citation}}});
    let segment = json!({"type":"object","additionalProperties":false,"required":["index","text","supported","claims"],"properties":{"index":{"type":"integer"},"text":{"type":"string"},"supported":{"type":"boolean"},"claims":{"type":"array","items":claim}}});
    json!({"type":"object","additionalProperties":false,"required":["language_ok","neutral","segments"],"properties":{"language_ok":{"type":"boolean"},"neutral":{"type":"boolean"},"segments":{"type":"array","items":segment}}})
}
