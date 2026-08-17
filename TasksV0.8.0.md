# TasksV0.8.0 — Inspiração Hermes Agent

> Backlog derivado da análise comparativa entre o Orbit e o
> [Hermes Agent](https://github.com/NousResearch/hermes-agent) (Nous Research, MIT).
> Mesmas regras de `Tasks.md` e `PlanejamentoMelhoriaPENDENTES.md`: execute na
> ordem, uma tarefa por vez, `cargo fmt` + `cargo clippy --all-targets -- -D warnings`
> + `cargo test --all-targets` antes de fechar cada uma. Comentários e nomes de
> tipos em inglês; este documento em português.
>
> Legenda de esforço: `P` pequeno (< 2h) · `M` médio (meio dia) · `G` grande (1–3 dias)

---

## Contexto

Os dois projetos resolvem problemas diferentes. O Orbit é um **cliente desktop
auditável para um repositório local**, com aprovação humana em todo write. O Hermes
é um **assistente ambiente que aprende**, espalhado por 22 plataformas de mensagem.
A análise não busca paridade — busca as capacidades do Hermes que reforçam a tese do
Orbit sem contradizê-la.

Quatro delas não têm equivalente no Orbit:

1. **Memória procedural (skills)** — o `.orbit/` guarda *o que foi decidido*
   (decisões, achados, tarefas). Não guarda *como se faz neste projeto*. Skills são
   markdown commitável, então encaixam na tese central do produto — trocar de modelo
   é handoff, não recomeço — sem mudar arquitetura.
2. **Recall entre sessões** — `Db::search_history` (`src/storage/db.rs:297`) já
   indexa tudo em FTS5, mas só alimenta a caixa de busca da UI. O agente não
   consegue perguntar "como resolvemos isso antes?".
3. **Extensibilidade** — `ToolRegistry::workspace_tools()` (`src/tools/mod.rs:153`)
   fixa 20 ferramentas em tempo de compilação. Em Rust, plugin dinâmico é dor;
   **MCP é a história de plugin correta**.
4. **Providers** — o trait `AiProvider` (`src/providers/mod.rs:182`) foi desenhado
   para múltiplos backends e nunca teve uma segunda implementação. Tudo passa pelo
   OpenRouter: ponto único de falha, e modelo local é impossível.

**Descartado deliberadamente:** gateway de mensageria (Telegram/Discord conflita
frontalmente com aprovação humana síncrona), cron scheduler, backends de sandbox
serverless (Modal/Daytona) e export de trajetórias para treino — o Hermes é
laboratório da Nous, o Orbit é produto.

**Onde o Orbit já é superior, e não deve mudar:** papel como catálogo de ferramentas
em vez de instrução (`src/session/roles.rs:1-6`), dupla checagem no dispatch
(`src/session/agent_loop.rs:262`), estágio `Verify` determinístico entre estágios de
LLM (`src/pipeline/verify.rs`), teto de gasto em USD que pausa e pergunta, e o
`.orbit/` legível e commitável.

**Invariantes que nada aqui pode quebrar:** teto de gasto por sessão; aprovação
humana antes de todo write e todo comando novo; papel como catálogo, não como
promessa; saída de ferramenta como dado não confiável; `.orbit/` editável à mão sem
derrubar o app.

---

## Ordem de execução

```
Bloco A  V8.1 skills ──→ V8.3 nudge
         V8.2 search_history

Bloco B  V8.4 allows_risk ──┬──→ V8.6 subagentes
                            └──→ V8.7 MCP        (Bloco C)
         V8.5 tools paralelas   (independente)

Bloco D  V8.8 providers         (independente, pode correr em paralelo)
```

Caminho recomendado: **V8.1 → V8.2 → V8.3** (maior retorno, sem risco arquitetural,
fecha a tese do produto) → **V8.4 → V8.5** (baratas, destravam o resto) →
**V8.6 → V8.7** → **V8.8** (a maior, e a única que é decisão de produto além de
técnica).

---

# Bloco A — Memória procedural

### V8.1 — Skills em `.orbit/skills/` `G`

- **Depende de:** —
- **Arquivos:** `src/context/skills.rs` (novo), `src/context/mod.rs`,
  `src/context/store.rs`, `src/context/digest.rs`, `src/tools/skills.rs` (novo),
  `src/tools/mod.rs`, `src/session/roles.rs`, `src/ui/coder/context_panel.rs`
- **Objetivo:** acumular procedimento, não só decisão. Uma skill escrita numa sessão
  com o Gemini fica disponível para o Claude na sessão seguinte.
- **Formato:** `.orbit/skills/<slug>/SKILL.md`, com frontmatter YAML mínimo:

  ```markdown
  ---
  name: rodar-testes-de-integracao
  description: Como rodar a suíte de integração, incluindo o setup do banco efêmero.
  ---

  <corpo em markdown>
  ```

  Diretório, não arquivo solto, por dois motivos: compatibilidade com o padrão
  agentskills.io — o usuário pode largar uma skill pronta ali — e espaço para
  arquivos de referência ao lado do `SKILL.md`. Esses arquivos vizinhos são lidos com
  o `read_file` existente: estão dentro do root do projeto, logo
  `security/paths.rs::resolve_within_root` já cobre. **Nenhuma superfície de
  segurança nova.**

- **Passos:**
  - `skills.rs`: `Skill { slug, name, description, body, path }` e `load_all(dir)`
    varrendo `.orbit/skills/*/SKILL.md`. Seguir a regra do `store.rs` — *"Hand-edited
    files must never crash the app"*: frontmatter ausente ou malformado vira
    `warnings.push(...)` e a skill é ignorada, nunca panic.
  - Adicionar `pub skills: Vec<Skill>` a `OrbitStore` (`store.rs:157`) e carregar em
    `reload()` (`store.rs:195`), junto dos demais parsers.
  - `ensure_layout()` cria `.orbit/skills/` vazio na primeira abertura.
  - **Digest com progressive disclosure** (`digest.rs::build_digest`): injetar apenas
    uma seção `Skills disponíveis (N):` com `- <name>: <description>`, nunca o corpo.
    É isso que torna a feature barata em token. Teto de 50 skills listadas
    (configurável em `DigestSettings`); acima disso, listar as 50 e avisar.
  - `tools/skills.rs`:
    - `read_skill(name)` — `ToolRisk::ReadOnly`. Retorna o corpo do `SKILL.md`. Erro
      claro com a listagem dos nomes válidos quando não encontra.
    - `create_skill(name, description, body)` — `ToolRisk::Mutating`. **Produz um
      `FilePatch` em `ctx.proposed_patches` exatamente como o `write_file`**, então
      cai no fluxo de aprovação de `agent_loop.rs:308-357` sem código novo: o usuário
      vê o diff da skill antes de ela existir. Sobrescrever uma skill existente usa o
      mesmo caminho — é assim que a skill se corrige durante o uso.
    - Validar `name` como slug (`[a-z0-9-]+`, ≤ 64 chars) antes de virar caminho.
  - Registrar as duas em `ToolRegistry::workspace_tools()` e estender
    `CANONICAL_TOOLS` de 20 para 22 (`roles.rs:147`).
  - Matriz de papéis (`roles.rs:71`): `read_skill` para os quatro papéis;
    `create_skill` para Coder, Tester e **Architect** — documentar procedimento é
    trabalho de Architect, e ele já escreve em `.orbit/` via `record_plan`. Reviewer
    fica só com leitura.
  - `context_panel.rs`: lista de skills, com clique abrindo o `SKILL.md` no viewer
    existente.
- **Pronto quando:** uma skill criada numa sessão com o modelo A aparece no digest da
  sessão com o modelo B; o corpo só entra no contexto quando `read_skill` é chamado
  (teste com 20 skills confirma que o digest cresce ~1 linha por skill);
  `create_skill` mostra diff e respeita Deny; um `SKILL.md` sem frontmatter gera
  warning e não derruba o app; Reviewer não recebe `create_skill` no catálogo nem
  consegue chamá-la pelo dispatch.

### V8.2 — `search_history`: expor o FTS5 ao agente `M`

- **Depende de:** —
- **Arquivos:** `src/tools/history.rs` (novo), `src/tools/mod.rs`,
  `src/storage/db.rs`, `src/session/agent_loop.rs`, `src/session/roles.rs`
- **Objetivo:** a infra já existe e está subutilizada. `Db::search_history`
  (`db.rs:297`) faz `MATCH` com `snippet()` sobre `history_fts`; hoje só a UI usa.
- **Passos:**
  - `ToolContext` (`tools/mod.rs:49`) não carrega o `Db`; `TurnDeps` carrega
    (`agent_loop.rs:44`). Adicionar `pub db: Option<Arc<Db>>` a `ToolContext` e
    propagar em `bind_ctx` (`agent_loop.rs:365`) e em `ToolContext::for_tests`.
  - **Escopo por projeto, obrigatório.** `search_history` hoje varre `chat` e
    `session` globalmente. Um agente rodando no projeto A não pode ver conversa do
    projeto B — é vazamento entre repositórios. Adicionar filtro por `project_id` e
    um `search_history_scoped(project_id, query, limit)`; a tool só usa esta
    variante. Chats do Chat Mode ficam **fora** do escopo do agente.
  - `search_history(query, limit)` — `ToolRisk::ReadOnly`, limit padrão 10, teto 30.
    Retorna `label da sessão · data · snippet`. Sem corpo inteiro: o agente pede o
    que quiser depois.
  - Registrar, estender `CANONICAL_TOOLS` (22 → 23) e liberar para os quatro papéis —
    é leitura pura.
  - Acrescentar uma frase ao `CODER_SYSTEM_PROMPT` (`agent_loop.rs:20`): consultar o
    histórico antes de reinventar solução já tomada.
- **Pronto quando:** um teste cria duas sessões em projetos diferentes com o mesmo
  termo e a busca do projeto A retorna só o hit de A; o agente resolve uma questão
  citando uma sessão anterior sem o usuário colar nada.

### V8.3 — Nudge de persistência `P`

- **Depende de:** V8.1
- **Arquivos:** `src/session/agent_loop.rs`
- **Objetivo:** o prompt já pede *"record decisions as soon as you make them"*, mas
  nada verifica. O Hermes cutuca periodicamente, e é por isso que a memória dele
  enche. Sem isso, V8.1 e o `.orbit/` ficam subutilizados.
- **Passos:**
  - Contar, dentro de `run_turn`, as chamadas de ferramenta do turno e se alguma foi
    `record_decision` / `add_finding` / `create_skill`.
  - Ao passar de 8 chamadas sem nenhuma delas, injetar **uma vez por turno** uma
    mensagem delimitada, no mesmo padrão do resumo de contexto
    (`context_window.rs:62 wrap_summary`): marcadores
    `<<<ORBIT_NUDGE>>> … <<<END_ORBIT_NUDGE>>>`, com texto fixo do runtime.
  - Nunca repetir no mesmo turno; nunca injetar em turno cancelado ou que estourou
    orçamento.
- **Pronto quando:** teste com provider simulado que faz 9 tool calls sem registrar
  nada recebe o nudge exatamente uma vez; um turno com `record_decision` na terceira
  chamada não recebe nudge.

---

# Bloco B — Concorrência e delegação

### V8.4 — `AgentRole::allows_risk` `M`

- **Depende de:** —
- **Arquivos:** `src/session/roles.rs`, `src/session/agent_loop.rs`
- **Objetivo:** pré-requisito de V8.6 e V8.7. A guarda de dispatch
  (`agent_loop.rs:262`) só protege nomes presentes em `CANONICAL_TOOLS`; tudo fora
  passa livre. Assim que existirem ferramentas dinâmicas (MCP) ou spawnadas, essa
  brecha vira furo de papel: um Architect chamaria uma tool MCP que escreve.
- **Passos:**
  - `pub fn allows_risk(self, risk: ToolRisk) -> bool`:

    | Papel | ReadOnly | Executing | Mutating |
    |---|---|---|---|
    | Coder / Tester | ✓ | ✓ | ✓ |
    | Reviewer | ✓ | ✓ | ✗ |
    | Architect | ✓ | ✗ | ✗ |

    Derivado da matriz existente em `allowed_tools()`, não inventado — conferir tool
    a tool antes de escrever.
  - Guarda de dispatch em duas camadas: nome canônico → matriz de nomes
    (comportamento atual, intocado); nome não canônico → `allows_risk(tool.risk())`.
  - Teste de invariante novo: para cada papel, toda tool canônica permitida satisfaz
    `allows_risk(tool.risk())`. Isso trava a matriz e o enum juntos.
- **Pronto quando:** a tabela acima é verificada por teste contra `allowed_tools()`;
  uma tool não canônica `Mutating` é recusada para Architect no dispatch mesmo
  estando registrada.

### V8.5 — Ferramentas ReadOnly em paralelo `M`

- **Depende de:** —
- **Arquivos:** `src/session/agent_loop.rs`
- **Objetivo:** o loop despacha tool calls em sequência. Quando o modelo pede 5
  `read_file`, são 5 idas e voltas serializadas.
- **Passos:**
  - No ponto em que o turno itera as tool calls, particionar o lote: **se e somente
    se todas** forem `ToolRisk::ReadOnly`, rodar com `futures_util::future::join_all`
    com concorrência máxima 4. Qualquer `Mutating`/`Executing` no lote → sequencial,
    como hoje.
  - Essa regra é o ponto inteiro da tarefa: preserva ordem de aprovação, ordem de
    aplicação de patch e determinismo. Não tentar coordenação por caminho (o que o
    Hermes faz) — complexidade sem retorno aqui.
  - As `ChatMessage::ToolResult` voltam **na ordem original das calls**, não na de
    conclusão, senão o histórico persistido fica não determinístico.
  - `AgentEvent::ToolStarted` de todas dispara antes; a UI já lida com múltiplos em
    voo.
- **Pronto quando:** teste com 4 `read_file` simultâneos conclui em ~1× a latência de
  um, não 4×; a ordem dos `ToolResult` casa com a das calls; um lote misto
  (`read_file` + `write_file`) continua sequencial e a aprovação aparece uma vez só.

### V8.6 — `spawn_subagent` `G`

- **Depende de:** V8.4
- **Arquivos:** `src/tools/subagent.rs` (novo), `src/tools/mod.rs`,
  `src/session/manager.rs`, `src/session/roles.rs`, `src/ui/coder/sessions.rs`
- **Objetivo:** investigar sem poluir o contexto do pai. Hoje só o usuário cria
  sessão, e a pipeline (`src/pipeline/mod.rs`) é uma cadeia fixa de 5 estágios. O
  ganho real é o pai receber **só a conclusão**, não as 30 tool calls.
- **Decisão de projeto — subagente é somente-leitura.** `role` restrito a `Architect`
  ou `Reviewer`. Isso elimina de uma vez o roteamento de aprovação de write vindo de
  uma sessão que o usuário não abriu, mantendo o benefício principal. Subagente que
  escreve fica fora do escopo da 0.8.0.
- **Passos:**
  - `spawn_subagent(role, task, model?)` — `ToolRisk::Executing`, logo passa pela
    aprovação existente: o usuário vê o papel, a tarefa, o modelo e o orçamento antes
    de qualquer gasto.
  - **Orçamento fatiado do pai, nunca somado.** O subagente recebe uma fração do
    `budget_usd` restante do pai (padrão 25%, configurável) e o gasto dele debita do
    mesmo contador. Sem isso, o teto por sessão — invariante do produto — é
    contornável por spawn.
  - Reusar o `Semaphore` de `session/manager.rs` (`DEFAULT_SESSION_SLOTS = 3`): o
    subagente ocupa um slot e o pai aguarda se não houver.
  - `max_iter` do subagente menor que o do pai (padrão 10 contra 25).
  - **Profundidade 1**: o `ToolRegistry` do subagente nunca inclui `spawn_subagent`.
    Sem isso, recursão infinita gastando dinheiro real.
  - O pai aguarda o `run_turn` do filho e recebe o texto final como `ToolOutcome`,
    truncado no `TOOL_OUTPUT_CHAR_LIMIT` normal.
  - Cancelar o pai cancela o filho: `CancellationToken` do filho derivado do pai.
  - UI: aba de sessão marcada como subagente, com o pai indicado e transcript
    visível — nada roda invisível.
  - Só Coder e Tester recebem `spawn_subagent` no catálogo.
- **Pronto quando:** um Coder delega "mapeie onde a autenticação é usada" a um
  Architect e recebe um resumo, com o contexto do pai crescendo poucos milhares de
  tokens em vez de dezenas; o gasto do filho aparece no medidor do pai; cancelar o
  pai mata o filho; o subagente não consegue spawnar outro; `spawn_subagent` pede
  aprovação antes de gastar.

---

# Bloco C — Extensibilidade

### V8.7 — Cliente MCP `G`

- **Depende de:** V8.4
- **Arquivos:** `src/mcp/mod.rs`, `src/mcp/client.rs`, `src/mcp/tool.rs` (novos),
  `src/tools/mod.rs`, `src/session/agent_loop.rs`, `src/security/policy.rs`,
  `src/ui/settings.rs`, `Cargo.toml`
- **Objetivo:** abrir o ecossistema sem escrever ferramenta nenhuma e sem plugin
  dinâmico em Rust.
- **Passos:**
  - Transporte **stdio + JSON-RPC 2.0** apenas nesta rodada (HTTP/SSE fica fora):
    `initialize`, `tools/list`, `tools/call`. Serialização com `serde_json`, que já
    está no projeto — não adicionar SDK.
  - Ciclo de vida do processo: **reusar `command-group`** (já em `Cargo.toml` para o
    `runner/`). Servidor MCP gera filhos; matar só o pai deixa órfão. Job Object no
    Windows, process group no Linux — o mesmo problema já resolvido em M1.5. Subir na
    abertura do projeto, derrubar ao trocar de projeto e ao fechar o app.
  - `McpTool { server, remote_name, schema }` implementando o trait `Tool`
    (`tools/mod.rs:91`). Nome exposto ao modelo: `mcp__<server>__<tool>` (duplo
    underscore para não colidir com `_` em nome de servidor).
  - **Classificação de risco — o ponto crítico.** Um servidor MCP não pode declarar o
    próprio risco. Padrão: **`ToolRisk::Executing`**, ou seja, toda chamada MCP passa
    por aprovação. O usuário pode marcar uma tool específica como `ReadOnly` nas
    Configurações depois de vê-la funcionar, e essa marcação é **local à máquina**
    (nunca em `.orbit/config.toml`).
  - **Configuração e confiança.** Servidores declarados em `.orbit/config.toml`, que
    é versionável — logo um repositório clonado pode trazer servidor malicioso.
    Reusar exatamente o padrão de `RunConfig` (M1.4): hash de `command + args + env`,
    **primeira execução em cada máquina exige aprovação explícita**, editar a entrada
    invalida o hash e re-dispara. A denylist de
    `security/policy.rs:233 is_absolutely_denied` continua valendo sobre o comando do
    servidor e não é sobreponível.
  - **Isolamento de falha:** `tokio::time::timeout` em toda chamada (padrão 30 s).
    Servidor que trava ou morre marca as tools dele indisponíveis e devolve erro ao
    modelo; o turno segue. Um MCP ruim nunca congela o Orbit.
  - Saída MCP entra no histórico com os marcadores `ORBIT_TOOL_RESULT` existentes — é
    dado não confiável de terceiro, exatamente o caso que os marcadores cobrem.
  - Configurações: lista de servidores, status (rodando / parado / falhou), tools
    descobertas com o risco de cada uma, toggle de habilitar por servidor.
- **Pronto quando:** um servidor MCP de referência (ex.: filesystem) sobe, suas tools
  aparecem no catálogo prefixadas e uma chamada pede aprovação; matar o servidor no
  meio de um turno devolve erro ao modelo sem travar; um `config.toml` clonado com
  servidor novo exige aprovação na primeira execução; um Architect não consegue
  chamar tool MCP `Executing`; fechar o app não deixa processo órfão (verificado nas
  duas plataformas).

---

# Bloco D — Providers

### V8.8 — Segunda e terceira implementações de `AiProvider` `G`

- **Depende de:** —
- **Arquivos:** `src/providers/chat_completions.rs` (novo),
  `src/providers/anthropic.rs` (novo), `src/providers/openai_compat.rs` (novo),
  `src/providers/openrouter.rs`, `src/providers/mod.rs`, `src/providers/catalog.rs`,
  `src/secure_store.rs`, `src/ui/settings.rs`, `src/ui/onboarding.rs`
- **Objetivo:** o trait (`providers/mod.rs:182`) foi desenhado para isso e nunca teve
  segunda implementação. Destrava modelo local (custo zero, código não sai da
  máquina) e remove o ponto único de falha.
- **Passos:**
  - **Extrair antes de somar.** `openrouter.rs` (1169 L) mistura o dialeto
    `/chat/completions` com o específico do OpenRouter. Extrair `encode_messages`, o
    decode de evento e o tratamento de erro para `chat_completions.rs`, parametrizado
    por `base_url`, headers e forma de auth. `openrouter.rs` vira uma configuração
    fina sobre ele. `sse.rs`, `accumulate.rs` e `retry.rs` já são agnósticos e não
    mudam. **Passo isolado, com a suíte verde antes de seguir.**
  - `openai_compat.rs`: um provider com `base_url` configurável cobre OpenAI, Ollama,
    LM Studio, vLLM e LiteLLM de uma vez. Maior retorno por linha do bloco.
  - `anthropic.rs`: `/v1/messages` é dialeto distinto (`system` fora do array,
    `content` sempre em blocos, `tool_use`/`tool_result` em vez de `tool_calls`,
    eventos SSE nomeados). Não force no molde de `chat_completions.rs`; implemente o
    trait direto. `system_cache_chars` mapeia limpo para `cache_control` nativo.
  - **Credenciais:** `secure_store.rs` guarda uma chave só. Passar a chavear por
    provider (`orbit:openrouter`, `orbit:anthropic`, …), migrando a chave existente
    para `orbit:openrouter` na primeira execução, sem pedir nada ao usuário.
  - **Catálogo:** `AiModel`/`ModelInfo` ganham `provider_id`; `catalog.rs` passa a
    agregar por provider. Para OpenAI-compatible local, descobrir via `/v1/models`.
    **Preço zero em modelo local:** garantir que `prompt_price: Some(0.0)` não
    tropece na lógica de orçamento — o medidor mostra `$0.00` e o cap nunca dispara,
    mas `max_iter` continua valendo como freio.
  - `supports_tools` por provider: modelo local sem tool calling precisa ser recusado
    na criação da sessão Coder com mensagem clara, não falhar no meio do turno.
  - UI: Configurações com uma aba por provider (chave, estado, botão Testar reusando
    `validate_key`); seletor de modelo agrupado por provider; onboarding continua
    oferecendo OpenRouter como caminho padrão — chave única e cobrança única
    continuam sendo o argumento do produto.
  - Testes: replay de SSE gravado como `const` para cada dialeto novo, no mesmo
    padrão dos testes atuais do OpenRouter; `wiremock` para os caminhos de erro.
- **Pronto quando:** a mesma sessão Coder roda contra OpenRouter, Anthropic direto e
  um Ollama local trocando só o modelo; a chave da versão anterior continua
  funcionando sem reconfiguração; um modelo local sem tool calling é recusado na
  criação da sessão; o medidor mostra `$0.00` para local sem quebrar o cap.

---

# Verificação

**Por tarefa**, antes de fechar:

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

**Ponta a ponta**, ao fim de cada bloco, em `src/e2e.rs` contra o `ScriptedProvider`
já existente — sem rede, determinístico, no padrão dos testes atuais
(`two_sessions_handoff_matches_digest`, `budget_cap_stops_the_turn`):

| Bloco | Cenário e2e |
|---|---|
| A | Sessão 1 (modelo A) cria skill → sessão 2 (modelo B) a vê no digest, chama `read_skill` e a aplica. Estende `two_sessions_handoff_matches_digest`. |
| A | Busca encontra sessão anterior do mesmo projeto e **não** encontra a de outro projeto. |
| B | Lote de 4 `read_file` conclui em paralelo com `ToolResult` na ordem das calls; lote misto permanece sequencial. |
| B | Coder delega ao subagente Architect; gasto debita do pai; cancelar o pai mata o filho; subagente não spawna. |
| C | Servidor MCP fake por stdio (binário de teste): descoberta, chamada com aprovação, timeout, morte no meio do turno. |
| D | `two_sessions_handoff_matches_digest` rodando com providers diferentes em cada sessão. |

**Manual, na aplicação real** (`cargo run --release`), por bloco:

- **A** — abrir o próprio Orbit, pedir ao agente que documente como rodar a tríade
  como skill; aprovar o diff; conferir `.orbit/skills/` no `git status`; abrir nova
  sessão com outro modelo e confirmar que ela aparece.
- **B** — pedir um mapeamento amplo do código e observar o contexto do pai no medidor
  de ocupação (`AgentEvent::ContextOccupancy`).
- **C** — configurar um servidor MCP real, confirmar aprovação na primeira chamada,
  fechar o app com o servidor no ar e verificar que não sobrou processo.
- **D** — subir um Ollama local, rodar uma sessão Coder inteira, confirmar `$0.00`.

**Regressão obrigatória:** `for_role_narrows_the_catalog_size` (`tools/mod.rs:285`)
tem contagens fixas 15/13/8/15 e quebra em V8.1 e V8.2 — atualizar junto, não depois.
O invariante `all_role_tools_are_canonical` (`roles.rs:233`) deve continuar verde
após cada extensão de `CANONICAL_TOOLS`.

---

# Resumo

| Item | Tamanho | Depende de |
|---|---|---|
| V8.1 Skills em `.orbit/skills/` | G | — |
| V8.2 `search_history` como tool | M | — |
| V8.3 Nudge de persistência | P | V8.1 |
| V8.4 `AgentRole::allows_risk` | M | — |
| V8.5 Tools ReadOnly em paralelo | M | — |
| V8.6 `spawn_subagent` | G | V8.4 |
| V8.7 Cliente MCP | G | V8.4 |
| V8.8 Providers nativos | G | — |
