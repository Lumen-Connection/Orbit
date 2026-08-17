# TasksV0.9.0 — Inspiração Grok Build

> Backlog derivado da análise comparativa entre o Orbit e o
> [Grok Build](https://github.com/xai-org/grok-build) (SpaceXAI, Apache 2.0).
> Mesmas regras de `TasksV0.8.0.md`: execute na ordem, uma tarefa por vez,
> `cargo fmt` + `cargo clippy --all-targets -- -D warnings` + `cargo test --all-targets`
> antes de fechar cada uma. Comentários e nomes de tipos em inglês; este documento em
> português.
>
> **Pré-requisito:** o V0.8.0 está implementado no working tree mas não commitado nem
> verificado. Rodar a tríade e commitar antes de começar qualquer item daqui.
>
> Legenda: `P` pequeno (< 2h) · `M` médio (meio dia) · `G` grande (1–3 dias)

---

## Contexto

O Grok Build é o concorrente direto do Orbit — também é um agente de código escrito em
Rust. A comparação expôs três lacunas concretas, todas em áreas onde o Orbit já tem a
fundação pronta e só falta a camada final.

1. **Subagente que escreve.** O V8.6 restringiu subagente a papéis somente-leitura
   (`src/tools/subagent.rs:21 parse_role`) porque não havia como rotear aprovação de
   write vinda de uma sessão que o usuário não abriu. O Grok Build resolve com
   `isolation: worktree` — o filho escreve num `git worktree` descartável, e o merge
   de volta é o único ponto de aprovação. Isso não relaxa o invariante do Orbit;
   **reforça**: em vez de 20 cliques de aprovação, um só, sobre o diff consolidado.
2. **Hooks de ciclo de vida.** O Grok Build tem `PreToolUse`/`PostToolUse` capazes de
   negar uma chamada antes do dispatch. O Orbit tem o ponto de acoplamento exato
   (`src/session/agent_loop.rs:244 dispatch_tool`) e já tem o padrão de confiança por
   fingerprint (`src/mcp/trust.rs`) para configuração versionável.
3. **Sandbox de kernel.** O [docs/security.md](docs/security.md) declara que não há
   jail — só contenção e política. O Grok Build usa Landlock no Linux e Seatbelt no
   macOS. O Orbit pode fechar a metade Linux dessa lacuna honestamente.

**Fora de escopo por decisão:** modo headless/CLI e ACP — reconhecidamente a lacuna
mais estrutural do Orbit, mas fora desta rodada; plugins com marketplace; LSP; slash
commands. E **nada de `bypassPermissions`**: a ausência de um modo que desliga
aprovação é característica do produto, não limitação.

**Invariantes que nada aqui pode quebrar:** toda alteração que chega à árvore de
trabalho do usuário passa por aprovação humana; teto de gasto por sessão; papel como
catálogo de ferramentas com dupla checagem no dispatch; `.orbit/` legível e
commitável; Windows como plataforma de primeira classe.

---

## Ordem de execução

Os três são independentes entre si. Ordem por valor:

```
V9.1 worktree  →  V9.2 hooks  →  V9.3 landlock
   (destrava)      (barato)       (plataforma-específico)
```

---

# V9.1 — Isolamento por worktree para subagente que escreve `G`

- **Depende de:** —
- **Arquivos:** `src/session/worktree.rs` (novo), `src/tools/subagent.rs`,
  `src/session/subagent.rs`, `src/workspace/patch.rs`, `src/ui/coder/approvals.rs`,
  `src/ui/coder/sessions.rs`, `src/app.rs`
- **Objetivo:** permitir `role: coder | tester` em `spawn_subagent`, com as escritas
  contidas num `git worktree` descartável e um único ponto de aprovação no retorno.

### A ideia central

O filho recebe um `Project` **enraizado no worktree**, não no repositório do usuário.
Com isso, `security/paths.rs:15 resolve_within_root` continua confinando exatamente
como hoje — só que contra outra raiz. **Nenhuma lógica de confinamento muda.**

Dentro do worktree o filho escreve sem prompt, porque escrever ali não toca a árvore do
usuário. O prompt acontece uma vez, no fim, sobre o diff consolidado.

### Passos

- `worktree.rs`:
  - `create(project, session_id) -> Result<Worktree, String>` executando
    `git worktree add --detach <path> HEAD`. Caminho **fora do root do projeto**:
    `storage::data_dir()/worktrees/<project_id>/<session_id>`. Dentro do root, o
    worktree seria varrido pelo `grep`/`glob` do pai e apareceria no `git status`.
  - `changes(&self) -> Vec<PathBuf>` via `git -C <wt> status --porcelain`.
  - `remove(self)` via `git worktree remove --force`, e `prune(project)` via
    `git worktree prune` chamado **na abertura do projeto**, para limpar órfãos de
    crash.
  - Reusar o helper `git()` de `src/tools/git.rs:21` — extrair para um módulo comum em
    vez de duplicar `Command::new("git")`.
- `spawn_subagent` ganha `isolation: "none" | "worktree"` (padrão `"none"`):
  - `parse_role` aceita `coder`/`tester` **se e somente se** `isolation ==
    "worktree"`. Papel que escreve com `isolation: none` é recusado com mensagem
    explícita — nunca degradação silenciosa para leitura.
  - `child_registry` ganha variante de worktree: mantém as tools de escrita, mas
    continua desregistrando `spawn_subagent` (profundidade 1) e **acrescenta
    `git_commit` e `git_push` à remoção** — o filho não commita nem publica.
  - `run_command` do filho **continua sujeito à allowlist** (`CommandPolicy`). O
    worktree isola o sistema de arquivos, não a execução; um comando ainda alcança a
    rede e o resto da máquina.
- **Merge de volta como aprovação única:** ao terminar, para cada arquivo alterado no
  worktree, construir um `FilePatch` contra o **conteúdo atual no root do pai**,
  reusando `workspace/patch.rs`. Se o arquivo mudou no pai enquanto o filho rodava, o
  patch volta `Conflicted` — comportamento já existente, sem código novo. Os patches
  entram na fila de aprovação normal (`ui/coder/approvals.rs`).
- **Árvore suja do pai:** o worktree nasce de `HEAD`, então alterações não commitadas
  do pai **não** estão nele. Não recusar por isso — avisar, em dois lugares: no diálogo
  de aprovação do `spawn_subagent` e numa linha do system prompt do filho. Sem isso o
  filho "conserta" um arquivo que ele vê diferente do que o usuário vê.
- **Projeto sem git:** `git rev-parse --git-dir` falha → recusar `isolation: worktree`
  com mensagem clara. Sem fallback silencioso.
- Ciclo de vida: remover o worktree ao concluir, ao cancelar e ao fechar o app.
  Cancelar o pai já cancela o filho (`ctx.cancel.child_token()`, já implementado) —
  garantir que o `remove` roda também nesse caminho.
- Orçamento fatiado (`SubagentHost::slice`) e `SUBAGENT_MAX_ITER` continuam valendo,
  sem alteração.
- UI: a aba do subagente indica o worktree e o papel; o painel de aprovação identifica
  que os patches vieram de um filho, com o label dele.

- **Pronto quando:** um Coder delega "implemente X e rode os testes" a um subagente
  Coder com `isolation: worktree`; o filho escreve e roda `cargo test` sem nenhum
  prompt de write; ao terminar, o usuário aprova **um** conjunto de patches; negar
  deixa a árvore do usuário intacta; um arquivo tocado pelos dois volta `Conflicted`;
  matar o app com filho ativo não deixa worktree órfão (verificado com
  `git worktree list`); `isolation: none` com `role: coder` é recusado; projeto sem git
  recusa worktree.

---

# V9.2 — Hooks `PreToolUse` e `PostToolUse` `G`

- **Depende de:** —
- **Arquivos:** `src/hooks/mod.rs`, `src/hooks/runner.rs`, `src/hooks/trust.rs`
  (novos), `src/session/agent_loop.rs`, `src/ui/settings.rs`, `docs/config.md`,
  `docs/security.md`
- **Objetivo:** deixar o projeto declarar guardas próprias — negar `write_file` em
  `migrations/`, rodar formatador depois de toda edição, notificar em `git_push`.

### Enquadramento honesto

**Hook não é fronteira de segurança.** A fronteira continua sendo a matriz de papéis, a
aprovação humana e a denylist absoluta. Hook é conveniência de política, e falha dele é
*fail-open* com aviso visível — um hook quebrado não pode inutilizar o app. Registrar
isso em `docs/security.md`; é a diferença deliberada em relação ao Grok Build, onde o
`PreToolUse` é a etapa 1 de um pipeline de autorização.

### Passos

- Ponto de acoplamento: `dispatch_tool` (`agent_loop.rs:244`). `PreToolUse` roda
  **depois** da guarda de papel (`agent_loop.rs:262`) e antes de `tool.execute`;
  `PostToolUse` em `finish_tool`. A guarda de papel nunca fica atrás de um hook.
- Contrato do runner — só shell nesta rodada; HTTP fica fora, é superfície de rede nova
  sem ganho num app local:
  - **stdin:** JSON `{ event, tool_name, arguments, session_id, role, project_root }`
  - **stdout:** JSON `{ decision: "allow" | "deny", reason?: string }`
  - **exit code ≠ 0** também significa `deny`, com `stderr` como motivo.
  - stdout vazio, ilegível ou timeout → **allow**, com warning na UI e no tracing.
  - Timeout de 10 s, morte pela **árvore de processos** via `command-group` (já em
    `Cargo.toml`), mesmo tratamento do `runner/`.
  - Ambiente do filho filtrado por `is_secret_env()` (`tools/shell.rs:247`) — hook não
    herda chave de API.
- `PostToolUse` **não pode alterar o resultado**, só observar. Alterar quebraria o
  determinismo do histórico persistido.
- Configuração em `.orbit/config.toml`, versionável:

  ```toml
  [[hooks]]
  event = "PreToolUse"
  matcher = "write_file|edit_file|multi_edit"   # regex sobre o nome da tool
  command = "python"
  args = ["scripts/guard.py"]
  ```

- **Confiança — reusar `src/mcp/trust.rs` sem reescrever.** O problema é idêntico ao
  dos servidores MCP: `config.toml` é commitável, logo um repositório clonado pode
  trazer hook malicioso. Generalizar `McpServerConfig::fingerprint` / `is_trusted` /
  `trust_on_this_machine` para um tipo comum de "comando declarado no projeto", com
  arquivo de confiança próprio (`hook_trust.json` em `storage::data_dir()`). Primeira
  execução em cada máquina exige aprovação explícita; editar a entrada invalida o hash.
  `is_absolutely_denied` vale sobre o comando do hook e não é sobreponível.
- Execução sequencial na ordem de declaração; **o primeiro `deny` vence** e os demais
  não rodam.
- Um `deny` aparece no transcript como resultado de erro da tool, com o motivo do hook
  — nunca silencioso, e o modelo precisa ver por que foi barrado.
- Configurações: lista de hooks, estado de confiança, último resultado, botão de
  desabilitar por hook.

- **Pronto quando:** um hook `PreToolUse` que nega escrita em `migrations/` barra o
  `write_file` e o motivo aparece no transcript; um hook que sai com código 1 nega; um
  hook que trava 30 s é morto em 10 s e a tool **prossegue** com warning; um hook novo
  vindo de `config.toml` clonado exige aprovação na primeira execução; um hook com
  comando da denylist é recusado sem opção de aprovar; a guarda de papel continua
  barrando antes de qualquer hook rodar.

---

# V9.3 — Landlock no Linux `G`

- **Depende de:** —
- **Arquivos:** `src/security/sandbox/mod.rs`, `src/security/sandbox/landlock.rs`,
  `src/security/sandbox/unsupported.rs` (novos), `src/tools/shell.rs`,
  `src/runner/process.rs`, `src/ui/settings.rs`, `docs/security.md`, `Cargo.toml`
- **Objetivo:** confinar por kernel o que **comandos filhos** alcançam no Linux. Fecha
  metade da lacuna que o `docs/security.md` admite.

### A restrição que decide o desenho

Landlock é **irrevogável e vale para a thread chamadora e seus filhos**. O processo da
GUI não pode ser restringido — ele precisa do keyring, de `~/.config`, do display.
Portanto o ruleset é aplicado **entre `fork` e `exec` do filho**, via
`std::os::unix::process::CommandExt::pre_exec` (bloco `unsafe`), não no processo do
app. Esse ponto define toda a tarefa.

### Passos

- Adicionar a crate `landlock` (~0.4) como dependência **só de Linux**, no bloco
  `[target.'cfg(target_os = "linux")'.dependencies]` já existente.
- Perfis, espelhando o Grok Build em escala menor:

  | Perfil | Leitura | Escrita |
  |---|---|---|
  | `off` (padrão) | irrestrita | irrestrita — comportamento atual |
  | `workspace` | irrestrita | root do projeto, `/tmp`, caches de toolchain (`$CARGO_HOME`, `~/.cache`) |
  | `strict` | root do projeto + caminhos de sistema | root do projeto, `/tmp` |

  Ambos os perfis restritos negam por construção `~/.ssh`, `~/.aws`,
  `~/.config/orbit` e o socket do keyring.
- Aplicar em **todo processo filho**: `tools/shell.rs` (`run_command`) e
  `runner/process.rs` (run configs long-running). Um perfil que cobre só um dos dois não
  confina nada.
- **Degradação tem que ser barulhenta.** Usar o modo `BestEffort` da crate para negociar
  a ABI, mas **reportar na UI a ABI efetivamente obtida**. Kernel < 5.13 → o ruleset não
  aplica nada; se a UI mostrar "workspace" nesse caso, ela está mentindo, e sandbox que
  mente é pior que sandbox ausente. Estado explícito: `Ativo (ABI n)` /
  `Indisponível: kernel < 5.13` / `Não suportado nesta plataforma`.
- **Windows:** fora de escopo, e o seletor de perfil precisa dizer isso — controle
  desabilitado com texto explicando, nunca um controle que não faz nada. Módulo
  `unsupported.rs` sob `#[cfg(not(target_os = "linux"))]` com a mesma assinatura.
- **Perfil congelado pela vida da sessão** (regra do Grok Build, e está certa): trocar no
  meio afrouxaria confinamento retroativamente. Persistir junto da sessão no SQLite e
  reaplicar ao restaurar.
- Configuração em `.orbit/config.toml` `[sandbox] profile = "..."` — commitável, logo
  **só pode apertar, nunca afrouxar** em relação ao padrão local da máquina definido nas
  Configurações. Um repositório clonado não pode desligar o sandbox de quem o clonou.
- Rede: a ABI v4 (kernel 6.7+) restringe `bind`/`connect` TCP. Tratar como bônus
  condicionado à ABI disponível, não como promessa do perfil.
- **`docs/security.md` precisa ser reescrito nesta tarefa.** Hoje afirma que não existe
  sandbox; depois disso a frase fica correta no Windows e incorreta no Linux. Deixar os
  dois casos explícitos.

- **Pronto quando:** com perfil `workspace` no Linux, um `run_command` que tenta ler
  `~/.ssh/id_rsa` falha com `EACCES` e o mesmo comando lendo um arquivo do projeto
  funciona (teste `#[cfg(target_os = "linux")]`, ignorado com mensagem quando a ABI não
  está disponível); um dev server iniciado por run config herda o mesmo confinamento; em
  kernel antigo a UI mostra `Indisponível`, não `Ativo`; no Windows o seletor aparece
  desabilitado e explicado; um `config.toml` clonado com `profile = "off"` não afrouxa o
  padrão local; `docs/security.md` descreve os dois sistemas operacionais corretamente.

---

# Verificação

**Por tarefa**, antes de fechar:

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

**Ponta a ponta**, em `src/e2e.rs` contra o `ScriptedProvider`, sem rede:

| Item | Cenário |
|---|---|
| V9.1 | Coder delega a um Coder com worktree; filho escreve 3 arquivos sem prompt; pai recebe 3 patches numa aprovação; Deny deixa a árvore intacta; arquivo tocado pelos dois volta `Conflicted`. |
| V9.1 | Cancelar o pai durante o filho remove o worktree (`git worktree list` limpo). |
| V9.2 | Hook que nega barra a tool e o motivo chega ao modelo; hook que trava é morto em 10 s e a tool prossegue; hook não confiado exige aprovação. |
| V9.2 | Guarda de papel dispara **antes** do hook: Architect chamando `write_file` é barrado sem o hook rodar. |
| V9.3 | `#[cfg(target_os = "linux")]`: filho lê arquivo do projeto e falha em `~/.ssh`. Ignorado com mensagem quando a ABI não existe. |

**Manual, na aplicação real** (`cargo run --bin orbit`):

- **V9.1** — abrir o próprio Orbit, delegar uma alteração real a um subagente Coder,
  conferir com `git worktree list` durante e depois, aprovar o diff consolidado.
- **V9.2** — escrever um hook de 5 linhas que nega escrita fora de `src/`, confirmar o
  bloqueio e a mensagem no transcript.
- **V9.3** — no Linux, perfil `workspace`, pedir ao agente que rode `cat ~/.ssh/id_rsa`
  e confirmar a negação; conferir a ABI mostrada nas Configurações contra `uname -r`.

**Regressões esperadas:** `child_registry_never_includes_spawn`
(`tools/subagent.rs:218`) assume filho somente-leitura e precisa ganhar o caso worktree
sem perder o caso atual. `for_role_narrows_the_catalog_size` (`tools/mod.rs`) não deve
mudar — nenhum item aqui acrescenta tool canônica.

---

# Resumo

| Item | Tamanho | Depende de |
|---|---|---|
| V9.1 Worktree para subagente que escreve | G | — |
| V9.2 Hooks `PreToolUse` / `PostToolUse` | G | — |
| V9.3 Landlock no Linux | G | — |

Os três são independentes. **V9.1 primeiro**: é o único que destrava capacidade nova em
vez de reforçar o que já existe, e é o que transforma o subagente de ferramenta de
investigação em ferramenta de execução.
