# Plan: Instance Identity & Deferred Reference Resolution

## Overview

Providers, models, capabilities and launchers point at each other by name: a
capability names a model, a model names a provider, a launcher lists the
capabilities it enables. When building an object, the constructor needs to map
each of those names into a corresponding object. 

The way this is implemented today:

- The constructor does the lookups, so `ConfigConstructable::new` takes
  the whole application configuration as input. In some cases it is used,
  in others it's completely ignored.
- The constructor cannot report a failure. It returns the object, not a
  result, so a name that resolves to nothing has nowhere to go and calls
  `panic!` (issue #90). This panic cannot be reached today, thanks to the
  pre-validation implemented in spec 0024, but is still in the code.
- Each lookup rebuilds a whole collection, takes the one object it wanted and
  discards the rest, warning about unrelated broken entries where nothing can
  act on them.
- What an object needs after construction is copied into it during
  construction. A model holds its provider's settings by value, which may
  become stale (#51).
- A capability's `ModelRequirement` is checked once, when setup picks the
  model. The requirement and the model's metadata both ship in the binary
  while the pairing sits in the user's config, so an upgrade can invalidate a
  configuration that `capability list` still calls healthy and that binds
  without complaint.

The collections underneath build every configured instance up front and lend
references, so resolving one name means building all of them, and an instance
cannot outlive the collection that built it.

## Proposal

Move the lookup out of the constructor's reach and into the collection that
owns the things being looked up. A constructor receives the object it needs
instead of the means to find it, and the collection does the finding: on
demand, once per name, and able to fail.

This one change fixes the issues associated with the first four items above:
#51, #58, #90. It also builds the foundation for fixing the issue highlighted by
the fifth which turns the configuration resilient to an upgrade moving either
the model definitions embedded in the binary or the requirements declared
against them.

**Give each constructor what its kind needs, not the configuration.** What
each kind needs is short enough to state in full:

```
  provider     nothing beyond its own settings
     ▲
     │   a model names one provider
     │
  model        its provider
     ▲
     │   a capability names one model
     │
  capability   a way to look up a model by name
     ▲
     │   a launcher enables several capabilities
     │
  launcher     nothing; capabilities are bound when a launch runs
```

Each registry declares the one thing its constructors require, so those four
lines become types. The argument that almost every constructor ignores
disappears, the two call sites that fake one with an empty configuration go
with it, and the compiler enforces which kind depends on what.

**Let construction fail.** A constructor that looks something up returns a
result, so a name that resolves to nothing produces a message. That removes
the `panic!` and with it the reason the pre-flight walk exists, leaving one
definition of a valid reference instead of two. The same change lets a
settings blob that does not parse be reported instead of silently replaced by
defaults, which is a separate known weakness.

**Build on demand and remember what was built.** Each collection keeps the
settings it was built from and constructs an instance the first time that name
is asked for, returning the same object every time afterwards.

Speed is not the reason. The configured data is small, and construction is
cheap, though not free: a provider constructor builds one or two reqwest
clients, each setting up a connection pool and TLS configuration. Two other
effects matter more. Building everything up front has to deal with a failing
entry inside a loop, and a loop can only warn and skip, which is why those
warnings reach people who did not ask for them; building on demand hands the
failure back to whoever asked, so the caller decides what to do with it. And
remembering what was built gives one object per name within a collection,
which removes the trap where looking a model up removes it from the
collection.

Identity is scoped to one configuration snapshot: a configuration change
produces a new collection, and the old one is discarded along with everything
it had built. The four collections are held together on the application
context and discarded as a set whenever configuration is written, which
includes a repair accepted from a remediation prompt. One collection per kind
per run means a name built for one caller is the object every later caller
gets.

**Hand a model its provider.** With the provider resolved by the collection
and passed to the constructor, the model holds it as a field. The copied
settings, the string key shared between two files, and the generated field go
away, and a model that was never configured is a name the collection does not
have rather than an object reporting a confusing error.

**Check the requirement where the name resolves.** Resolving a capability's
model is the one step that holds both the declaration of what the capability
needs and the model that was built for it, so that is where the two are
compared. The check runs whenever the capability is built, rather than once
when setup picked the model, so an upgrade that moves either the requirement
or the model's metadata is reported instead of reaching a launch.

Checking whether a name resolves stays where spec 0024 put it. Asking whether
names resolve is a different job from building things: a list command
annotates broken entries without constructing anything, and a removal warns
about what it would strand. That remains a plain read of configuration.

### How a build runs

Building a capability recurses through the same shape at each level: look in
the cache, find the settings, build with what this kind needs.

```
CapabilitySource::get("chat")
   │
   ├─ already built? ────────────────────────> the same Arc as last time
   │
   ├─ settings for "chat"? ── no ──> Err: capability 'chat' is not configured
   │
   └─ build it, with what a capability needs: a model lookup
         │
         AgentModelCapability::new("chat", settings, models) -> Result
            │
            ├─ settings do not parse ──> Err
            │
            └─ models.get("granite")
                  │
                  ├─ already built? ─────────> the same Arc as last time
                  │
                  ├─ settings for "granite"? ── no ──> Err: not configured
                  │
                  └─ build it, with what a model needs: its provider
                        │
                        ├─ providers.get("ollama")
                        │     ── no ──> Err: model 'granite' names provider
                        │                    'ollama', which is not configured
                        │
                        └─ GraniteModel::new("granite", settings, provider)
   │
   ├─ Err ──> reported; nothing cached, nothing handed out
   └─ Ok  ──> cached and returned
```

Nothing in that chain sees the application configuration. A failure anywhere
in it stops the build and is reported by the collection that started it, which
is the behaviour issue #90 asks for, reached without a crash and without a
second walk agreeing that the crash was avoidable.

### Where the configuration goes

Configuration is not threaded down a chain of constructors. It is read once
and split four ways, each collection taking its own kind's settings and none
of them keeping the whole:

```
  configuration ──┬──> launcher collection     the configured launchers
                  ├──> capability collection   the configured capabilities
                  ├──> model collection        the configured models
                  └──> provider collection     the configured providers
```

Separately, a collection holds a handle to the collection it needs to ask,
which is who-can-ask-whom rather than who-owns-what:

```
  capabilities ──asks──> models ──asks──> providers
```

So one object is never handed another object's settings. A capability holds
the name `"granite"`; that name is a key into settings the model collection
already has, taken from the same configuration at the same moment. This is
what answers "if a launcher only gets its own settings, where do its
capabilities get theirs" — from the capability collection, never from the
launcher.

### Where checking sits

Spec 0024's checking runs beside this, not inside it. It answers what would
happen without building anything, which is what a list annotation, a
remove-time warning and a repair prompt all need. Building answers what did
happen, and reports its own failures.

```
  a command
     │
     ├──> check      reads configuration only, builds nothing
     │               -> list annotations, repair prompts, removal warnings
     │
     └──> build      constructs, resolves, and reports its own failures
```

Today the capability collection runs the check before building, because
building cannot report a failure. Once it can, that call goes, and the two
stop having to agree on what counts as valid.

Both do ask configuration the same question: which names does this entry
point at. Spec 0024 answers it in one place, four small implementations that
each return the names their kind holds. That stops being private to the
checking code and becomes the single declaration of an entry's outbound names,
read by the checker, by the removal warning, and by the collections.

## Alternatives Considered

**Construct first, wire references up in a second step.** A capability would
be built with its model unset, and a separate call on the `Capability` trait
would fill it in from a lookup. Rejected for three reasons. It allows a
half-built object the type system cannot rule out, so every user of it has to
handle a state that should not exist. It puts the lookup back into the
`Capability` trait, which spec 0017 removed from `bind` because an abstract
interface should not name the collection type its implementations happen to be
stored in. And spec 0017 recorded a decision that a capability's model must
not be optional, because a capability without a model is not useful; a second
step makes it optional again.

**Resolve on first use behind a lazy accessor.** A capability would hold a
resolver and its model's name, and resolve the first time the model is read,
keeping the constructor infallible. Rejected because it does not remove the
problem it is meant to solve: the object still has to be handed a resolver
from outside at construction, so the injection question is unchanged. It also
moves the failure to the latest possible point, which for a capability is
during a launch, and it makes the model optional again, against the spec 0017
decision above.

**One shared resolver type for every registry.** A single trait, passed to
every constructor, that can look up a model or a provider. Simpler than a type
per registry, but providers, launchers and output backends would go on
accepting an argument they ignore, which is the situation being removed. A
construction type per registry costs one parameter on the factory macro and
leaves no ignored arguments.

**Give launchers reference resolution too.** A launcher lists the capabilities
it enables, so it looks like the same problem one level up. It is not: a
launcher holds no capabilities. Binding one is an asynchronous operation that
mutates the launcher, with setup before it and teardown after, so it is an
action taken when a launch runs, not a field being filled. The list of enabled
capabilities is a selection resolved at launch time, which is where the launch
command already resolves it, and the launcher collection exists so `launcher
list` and the setup wizard can enumerate launchers without launching one.

**Declare every kind's references in registry metadata.** Capabilities already
declare theirs there, because a capability stores its model's name inside its
own settings blob and something has to say which key holds it. A launcher's
enabled capabilities and a model's provider name are typed fields, so
declaring them the same way would mean either moving them into a settings blob
or having metadata name an accessor, which is what the four existing
implementations already are.

## Out of Scope (Future Work)

- A process-global memoised instance cache (#58's `construct_shared`).
  Identity here is scoped to a configuration snapshot rather than to the
  process. Sub-Task 2 is the eagerness half of #58, which spec 0024 left in
  place and worked around from the command layer.
- `ConfigConstructable::new` returning `Result` so a malformed instance config
  surfaces instead of becoming a default (#59 item 2). This is the other
  failure inside `new`, and the one relocating reference resolution does not
  touch: `serde_json::from_value(cfg).unwrap_or_default()` still swallows a
  config blob that does not parse. Spec 0024's note that fixing #90 requires a
  fallible `new` applies to that failure, not to the panic.
- A shared `#[cfg(test)]` test-support module (#59 item 4) and the
  `#[allow(unused)]` cleanup in `define_factory!` (#59 item 3).
- User-facing detection and remediation of dangling references. Spec 0024
  shipped it: `validate_ref` and `find_dangling` over config and registry
  metadata, and a shared remediation prompt driven by the list, info, launch
  and remove commands. This plan changes where a reference is resolved, not
  what the user is shown when one is broken.
- Runtime liveness (#36).

---

## Sub-Tasks

Sub-Tasks 1 and 2 change how instances are held. 3 and 4 change when
references are resolved. 5 removes the argument that is then unused.

---

### Sub-Task 1 — Sources hold and hand out `Arc`

**Intent**
Give a configured instance one owner that can share it, so the same id always
yields the same object within a source.

**Expected Outcomes**

Each of the four `*Source` types stores `Arc` rather than `Box` in its
`constructed` vector, and `Configured::instances()` returns owned handles:

```rust
fn instances(&self) -> Vec<(String, Arc<U>)>;
```

`dependency::resolve` passes `&*instance` to `admits_instance`, and the
production call sites that iterate `instances()` bind an `Arc` instead of a
reference. Sources still construct everything up front at this point, so the
only observable change is that callers can hold an instance past the source's
borrow.

`ModelSource::take` becomes `ModelSource::get`, returning the same `Arc` on
every call for a given id rather than removing the entry. It also loses its
`configured_variant` argument, which it took only to compute a proxy route
key; Sub-Task 3 moves route registration to the launch path, and the variant
it needs there is `ModelConfig.variant`.

Tests cover: two calls to `instances()` on one source returning pointer-equal
`Arc`s for the same id; `get` called twice for one model id returning
pointer-equal `Arc`s, replacing the existing test that asserts the second
`take` returns `None`; and the existing proxy round-trip test, moved to drive
registration from the launch path, still confirming one route per model.

**Relevant Context**
- `src/dependency/mod.rs:67-79` (`Configured`), `:113-137` (`resolve`)
- `src/models/mod.rs:29-121` (`ModelSource`, `take`), `:127-131` (`instances`)
- `src/providers/mod.rs:29-70`, `src/capabilities/mod.rs:27-86`,
  `src/launchers/mod.rs:31-70`
- `src/models/mod.rs:302-303` (the test that asserts a second `take` is `None`)
- `src/commands/model.rs:283,415,553,745,803`, `capability.rs:432`,
  `launcher.rs:337`, `setup.rs:1487`, `utils/ui/app.rs:146`

**Status** — `[ ]` not started

---

### Sub-Task 2 — Sources resolve by id, lazily

**Intent**
Build only the instance that was asked for, so one capability's model does not
drag in every other configured model, and no command reports a problem in a
part of the configuration it was not asked about.

**Expected Outcomes**

Each source keeps the configuration map it was built from alongside a cache:

```rust
pub struct ModelSource {
    configs: HashMap<String, ModelConfig>,
    providers: Arc<ProviderSource>,
    cache: Mutex<HashMap<String, Arc<dyn Model>>>,
    model_proxy: Option<ProxyHandle>,
}

impl ModelSource {
    pub fn get(&self, model_id: &str) -> anyhow::Result<Arc<dyn Model>>;
}
```

`get` returns a cached instance if present, constructs and caches it
otherwise, and returns an error naming the id when the configuration has no
such entry or its type name is unknown. `instances()` builds everything,
because a caller asking for all of them wants all of them, and
populates the same cache so a later `get` reuses what was already built.

`Mutex` rather than `RefCell`, because sources are held across await points in
the command layer and `Configured` implementations must stay `Sync`.

Providers, capabilities and launchers get the same treatment. `from_config`
keeps its signature, now recording configuration instead of constructing from
it.

Two behaviours that a source has today at build time move to first use, and
both must survive the move. A source that cannot construct an entry warns
about it once, at `from_config`; lazily there is nothing to warn about until
something asks. And `CapabilitySource::from_config` leaves a broken capability
out of `instances()` altogether, which is what keeps a dangling instance out of
the setup wizard's selection lists — spec 0024 records that as the reason its
"broken candidates in setup" item could stay out of scope. Filtering at
`instances()` rather than at `from_config` preserves both.

Tests cover: building a source over two configured models, calling `get` for
one, and confirming only that one was constructed (asserted through a cache
accessor); `get` for an id absent from the configuration returning an error
that names the id rather than panicking; `get` for a configured id whose type
name is unknown returning an error; `instances()` followed by `get` returning
the pointer-equal instance; and a config with one healthy and one
unconstructable model, confirming a `get` for the healthy one warns about
neither and `instances()` warns about and omits the broken one.

**Relevant Context**
- `src/models/mod.rs:35-67` (eager `from_config`), `:49-55` (the per-model
  warning that today fires for models nobody asked about)
- `src/models/base.rs:256` (`ModelSource::from_config` rebuilt per resolve)
- `src/capabilities/mod.rs:32-74`, `src/providers/mod.rs:34-58`,
  `src/launchers/mod.rs:36-60` (the three parallel eager loops)
- `docs/specs/0024-config-integrity-validation.md` (Out of Scope: broken
  candidates in `setup` selection lists)

**Status** — `[ ]` not started

---

### Sub-Task 3 — A model's provider comes from `ProviderSource`

**Intent**
Let a model reach its provider through the source that owns providers, rather
than carrying a copy of that provider's configuration.

**Expected Outcomes**

`ModelSource` holds an `Arc<ProviderSource>` built from the same `Config`, and
answers the model-to-provider question itself:

```rust
pub fn provider_for(&self, model_id: &str) -> anyhow::Result<Arc<dyn Provider>>;
```

It reads `ModelConfig.provider_id`, which spec 0024 made a required field, so
a configured model always names a provider and the only thing that can be
wrong is that the name resolves to nothing:

```
model 'x' is not configured
model 'x' names provider 'ollama', which is not configured
```

Three things go away. The `cfg["provider_config"]` injection in
`ModelSource::from_config`. The `provider_config` field and its
deserialisation in the generated model struct, along with the string key
`"provider_config"` that `src/models/mod.rs` and `build.rs` have to agree on
without the compiler checking it. And `Model::provider_config()` and
`Model::provider()`, the latter of which reaches from the model layer into
`PROVIDER_REGISTRY` with a `Config::default()` and builds a fresh provider on
every call.

Removing `Model::provider()` removes the seam the session proxy hooks into.
`ProxiedModel` overrides exactly that one method, returning a `ProxiedProvider`
pointed at the local proxy instead of the real upstream, and that is how
`resolve_provider_endpoint` and the launch overlay reach the proxy today.

The proxy moves down a layer. `ProviderSource` holds the handle and returns
providers whose connection details point at the local proxy, which is all
`ProxiedProvider` ever did: it swaps a base URL and delegates the rest,
including `model_alias`, so nothing about it is specific to one model.
`ProxiedModel` is deleted, and `provider_for` returns whatever the provider
collection gives it without knowing whether a proxy is running.

Route registration does not move with it. It needs the model, its variant, the
real provider's connection details and the handle at once, and a provider
collection knows nothing about models. It moves instead to the launch path,
which Sub-Task 4 already establishes as the one place the handle is live: after
resolving its capabilities, `run_launch` registers one route per model it
resolved. That also settles the sequencing, since the real upstream details the
route needs are read before the swap rather than from behind it.

`ConfiguredModel` gains the resolved provider alongside the model, both
obtained from the source when the `ConfiguredModel` is built, so
`resolve_provider_endpoint` reads a field instead of calling
`self.model.provider()`. `ModelSource::get`'s proxy-route registration uses
`provider_for` for the same purpose, as do the three command-layer callers
that reach a provider through a model today.

Tests cover: `provider_for` returning the configured provider for a healthy
model; the two failure messages above, checked separately, confirming a
catalog id that was never configured is distinguished from a configured model
whose provider is gone, which `Model::provider()`'s single "model has no
configured provider" could not express; the existing
`model_source_resolves_provider_from_provider_id` and
`model_source_provider_errs_when_provider_id_unresolvable` tests rewritten
against `provider_for`; and, with a proxy handle active, `provider_for`
returning connection details pointed at the local proxy while
`health_check`/`pull_model` still reach the real upstream.

**Relevant Context**
- `src/models/mod.rs:38-48` (the sidecar injection), `:94` (route registration
  through `model.provider()`)
- `src/models/base.rs:161-183` (`provider_config`, `provider`)
- `build.rs:60-63,72-75` (generated field and its deserialisation)
- `src/models/base.rs:298-336` (`resolve_provider_endpoint`), `:311` (the call)
- `src/proxy/model_wrapper.rs:19-88` (`ProxiedModel`), `:91-110`
  (`ProxiedProvider`)
- `src/config/mod.rs:71` (`ModelConfig.provider_id`, now required)
- `src/commands/model.rs:748`, `:819`, `src/commands/capability.rs:452`
  (command-layer callers of `Model::provider()`)

**Status** — `[ ]` not started

---

### Sub-Task 4 — A capability's model is resolved after construction

**Intent**
Separate building a capability from wiring it to the model it names, so the
construction path reports a missing model itself rather than depending on a
separate walk having run before it.

**Expected Outcomes**

The models layer publishes the narrow view a capability needs, which
`ModelSource` implements:

```rust
pub trait ModelLookup: Sync {
    fn resolve(&self, model_id: &str) -> anyhow::Result<ConfiguredModel>;
}
```

`ConfiguredModel` stays the struct it is today, a model plus the variant the
user pinned, gaining the provider Sub-Task 3 takes off the model. What changes
is who assembles it: `ModelSource` does, out of its own three reads, so the
`&Config` argument `ConfiguredModel::resolve` takes today has no successor.

`Capability` gains one optional method, defaulting to a no-op so the
capabilities with no outbound references need no changes:

```rust
fn resolve_refs(&mut self, models: &dyn ModelLookup) -> anyhow::Result<()> { Ok(()) }
```

`Validatable::refs` moves out of `src/config/validation.rs` and stops being
private, so an instance's outbound references have one declaration that the
validator's walk, the remove-time dependents scan and the sources all read.
Its four implementations are unchanged; only their visibility is. Extending
`CapabilityMetadata.dependencies` to the other three kinds is not part of
this: `Dependency.config_key` exists to name a key inside opaque JSON, and a
launcher's `enabled_capabilities` and a model's `provider_id` are typed
fields, so declaring them in metadata would mean either moving them into a
config blob or having metadata name an accessor, which is what the four
implementations already are.

The six model-backed capabilities store `Option<ConfiguredModel>`, set it in
`resolve_refs`, and read it in `bind`. They are written in four places: the
two plain implementations for `agent-model` and `vision-mcp`, and the two
macros in `sub_agent.rs` that generate the other four types between them.

`resolve_refs` checks that the model it found satisfies what the capability
declared it needs, not only that the model exists. Each model dependency in
`CapabilityMetadata.dependencies` already carries a `ModelRequirement`, and
that type already implements `Requirement<dyn Model>`, whose `admits_instance`
is the call the setup picker makes when it filters candidate models.
Resolution makes the same call, so a model that does not satisfy the
requirement fails resolution with the capability, the model and the unmet
requirement named.

`ConfiguredModel::resolve_provider_endpoint` then loses its
`required_function` parameter. That parameter is there to check that the model
supports the function the capability needs, which is the fact
`ModelRequirement.supported_functions` states and resolution now checks.
`endpoint_function` stays: it selects which endpoint to look up rather than
stating what the model must support, and the two differ, since
`vision-mcp` requires `ImageUnderstanding` of its model and looks the endpoint
up via `Chat`. The check that the provider supports the request's `api_type`
stays too, because `api_type` arrives on the `BindingRequest` and is not
known before one.

Two production paths construct a capability, and both call `resolve_refs`.
`CapabilitySource` calls it on each freshly constructed capability, whether
that construction came from `get` or from `instances()`, and on `Err` skips
that capability with a warning naming both the capability and the model it
wanted, which is the outcome its `validate_ref` gate produces today.
`run_launch` builds its enabled capabilities through the registry directly
rather than through the source, so it calls `resolve_refs` there too. That is
the one place the session proxy handle is live, which is why Sub-Task 3 puts
route registration there: once its capabilities resolve, `run_launch` has every
model it needs to route.

Spec 0024's `validate_ref` gate in `CapabilitySource::from_config` goes. It
is there because construction cannot report a failure, and `resolve_refs`
returning `Err` now does, with the same outcome: skip the capability and warn.
Keeping it would also put an eager transitive walk over the whole
configuration inside a source Sub-Task 2 has just made lazy. The validator
keeps every one of its command-layer callers, none of which construct
anything: `find_dangling` for the list annotations, `dependents` for the
remove-time scan, and `validate_ref` for the remediation prompt and the launch
prelaunch check.

`ConfiguredModel::resolve` becomes fallible and takes the source rather than
the whole configuration. Its `panic!` is deleted, which closes #90 in the code
as well as in the behaviour spec 0024 already delivered by keeping both
production paths away from it.

`Capability::bind` keeps the signature spec 0017 gave it, so binding still
needs nothing but the request.

Tests cover: a capability configured against a removed model, asked for
through both `get` and `instances()`, producing an error and an omission
rather than a crash, with the healthy capability alongside it still present;
the same configuration driven through `run_launch` with the 0024 prelaunch
check stubbed out, confirming the launch path reports the broken capability
itself rather than reaching the deleted panic; `resolve_refs` returning an error that
names both the capability and the missing model; a capability whose model is
configured but whose model's provider is gone, confirming the error names the
provider rather than stopping one hop short; `bind` on a capability whose
`resolve_refs` was never called returning an error rather than unwrapping
`None`; a capability whose model exists but does not satisfy its declared
`ModelRequirement` failing resolution, naming the unmet requirement, where the
same model resolves for a capability that does not require it; and, per
capability type, that every id `refs()` reports for it is one `resolve_refs`
consumes, so the `config_key` the metadata declares and the field
`resolve_refs` reads cannot drift apart.

**Relevant Context**
- `src/models/base.rs:240-266` (`ConfiguredModel::resolve`, the panic at 260)
- `src/capabilities/agent_model.rs:47-60`, `vision_mcp/mod.rs:96-110`,
  `sub_agent.rs:48-62` (`declare_sub_agent_basic`, three types) and `:192-207`
  (`declare_sub_agent_full`, one type)
- `src/capabilities/base.rs:264-289` (`Capability`)
- `src/capabilities/mod.rs:32-74` (spec 0024's `validate_ref` gate, removed here)
- `src/config/validation.rs` (the walk, which keeps its command-layer callers)
- `src/main.rs:786-800` (`run_launch`'s own capability construction loop)
- `src/commands/launcher.rs:250-267` (`prelaunch`, which is why the panic is
  already unreachable on the launch path)
- `src/capabilities/requirement.rs:69-91` (`Requirement<dyn Model>` for
  `ModelRequirement`)
- `src/dependency/mod.rs:113-123` (`resolve`, the only production
  `admits_instance` call today)
- `src/capabilities/agent_model.rs:121-136` (the declared `ModelRequirement`)
- `src/models/base.rs:298-337` (`resolve_provider_endpoint`; the
  `required_function` check at 318-321, removed here)
- `docs/specs/0017-remove-models-argument-from-bind.md`
- `docs/specs/0024-config-integrity-validation.md` lists checking that a
  referenced model satisfies its requirement as out of scope; this sub-task
  covers it.
- PR #91 addressed the same panic with `Capability::is_healthy()` and was
  closed. #90 stays open until the panic is deleted here.

**Status** — `[ ]` not started

---

### Sub-Task 5 — `ConfigConstructable::new` drops the global config

**Intent**
Remove the argument now that nothing reads it, and give the one remaining
consumer an explicit route.

**Expected Outcomes**

The trait method and both generated factory methods lose the parameter:

```rust
fn new(instance_id: &str, cfg: &serde_json::Value) -> Self;

pub(crate) fn construct(
    &self,
    name: &str,
    instance_id: &str,
    cfg: &serde_json::Value,
) -> Result<Box<dyn $trait>, String>;
```

Every implementation loses it too, including the generated one in `build.rs`
and the four `Ui` backends, whose factory takes a config it has never had
anything to put in.

`ClaudeLauncher` is the one production consumer that is not a capability. Its
`model_proxy` moves to `LaunchContext`, which `wire_model_proxy` already
receives and which `main` already builds immediately after starting the proxy
server. `Config.model_proxy` goes with it: `ProviderSource` receives the handle
when the launch builds it, `ModelSource` never sees one, and no constructor
reads it from configuration.

The two `Config::default()` placeholders disappear: `construct_ui` in `main`
and `Model::provider()`, the latter already deleted in Sub-Task 3.

Tests cover: the existing `instance_id_round_trips_from_construction` tests
across the seven launchers, updated for the new arity, confirming `Named`
still reports the configured id; `wire_model_proxy` driven from a
`LaunchContext` carrying a handle, producing the same overlay it produces
today from the field; and a grep-level assertion in review rather than in code
that no `impl ConfigConstructable` mentions `Config`.

**Relevant Context**
- `src/registry/mod.rs:17-40` (trait), `:113,146-147,233-245` (macro)
- `build.rs:62-78` (generated implementation)
- `src/main.rs:335-347` (`construct_ui`), `:765`, `:786-800` (launch path)
- `src/launchers/claude.rs:60-75`, `:262`, `:324` (`wire_model_proxy`)
- `src/launchers/base.rs:267-277` (`LaunchContext`)
- `src/config/mod.rs:49-56` (`model_proxy`)

**Status** — `[ ]` not started
