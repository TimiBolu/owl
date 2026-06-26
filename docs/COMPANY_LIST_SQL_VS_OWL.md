# Complex Company Listing: SQL vs Pure Owl

This note translates the pasted SQL into a pure-Owl style and explains the difference in mindset.

The SQL query is a serious reporting query. It is not a toy query. It does all of the following at once:

- starts from `org_companies`
- derives many scoped summaries per company
- joins in profile data from `companies`
- folds in findings/signals, controls, questionnaires, engagements, tags, assets, custom fields, inherent risk categories, parent company data, and reassessment metadata
- applies an org-scoped lifecycle filter
- returns one enriched company row per `org_company`

In SQL, this naturally becomes a tower of CTEs plus a large final `select`.

In Owl, the ideal is different.

The ideal is:

- start from the collection you want to return
- enter a scoped block
- define local derived collections and local derived summaries once
- finish with a single `select`

That is much closer to how the user thinks about the query.

## 1. What the SQL is really doing

At a high level, the SQL is:

1. compute many per-company summary datasets
2. compute many per-company array/object datasets
3. join them all back onto `org_companies`
4. filter the final set by org and lifecycle mode
5. emit one enriched company object

That is why the query has so many CTEs. SQL needs to stage the work explicitly.

## 2. What pure Owl would try to do instead

Pure Owl should center the query on the thing being returned.

Here, that is `org_companies`.

So the outer form should be:

```oql
org_companies {
  where { .org.id == orgId }
  where {
    discovery
      ? .lifecycle == "COMPANY_LIFECYCLE_DISCOVERY"
      : .lifecycle != "COMPANY_LIFECYCLE_DISCOVERY"
  }

  ...local derived collections and summaries...

  select { ...final enriched company object... }
}
```

This is the core Owl difference.

Instead of:
- global staging with many named SQL CTEs

we want:
- local scoped derivation close to the entity being returned

## 3. A pure Owl shape

The exact field names below follow the SQL closely, but the structure is Owl-native.

```oql
org_companies {
  where { .org.id == orgId }
  where {
    discovery
      ? .lifecycle == "COMPANY_LIFECYCLE_DISCOVERY"
      : .lifecycle != "COMPANY_LIFECYCLE_DISCOVERY"
  }

  company = .company
  parent_company = .parent

  questionnaires = .ongoing_questionnaires.where {
    .assessment_ongoing == true
  }

  ongoing_questionnaires = questionnaires.where {
    .ongoing == true
  }

  visible_signals = .signals.where {
    .visible == true
    && .work_status in ["FINDING_STATUS_OPEN", "FINDING_STATUS_ONGOING"]
    && .is_false_positive == false
    && exists(.winning_rule_id)
  }

  critical_signals = visible_signals.where {
    .resolved_level == "RISK_LEVEL_CRITICAL"
    && .org_level == "RISK_LEVEL_UNSPECIFIED"
  }
  org_critical_signals = visible_signals.where {
    .org_level == "RISK_LEVEL_CRITICAL"
  }

  high_signals = visible_signals.where {
    .resolved_level == "RISK_LEVEL_HIGH"
    && .org_level == "RISK_LEVEL_UNSPECIFIED"
  }
  org_high_signals = visible_signals.where {
    .org_level == "RISK_LEVEL_HIGH"
  }

  medium_signals = visible_signals.where {
    .resolved_level == "RISK_LEVEL_MEDIUM"
    && .org_level == "RISK_LEVEL_UNSPECIFIED"
  }
  org_medium_signals = visible_signals.where {
    .org_level == "RISK_LEVEL_MEDIUM"
  }

  low_signals = visible_signals.where {
    .resolved_level == "RISK_LEVEL_LOW"
    && .org_level == "RISK_LEVEL_UNSPECIFIED"
  }
  org_low_signals = visible_signals.where {
    .org_level == "RISK_LEVEL_LOW"
  }

  visible_risk_insights = .risk_insights.where {
    .visible == true
    && .status in ["FINDING_STATUS_OPEN", "FINDING_STATUS_ONGOING"]
  }

  critical_risk_insights = visible_risk_insights.where { .severity == "RISK_LEVEL_CRITICAL" }
  high_risk_insights = visible_risk_insights.where { .severity == "RISK_LEVEL_HIGH" }
  medium_risk_insights = visible_risk_insights.where { .severity == "RISK_LEVEL_MEDIUM" }
  low_risk_insights = visible_risk_insights.where { .severity == "RISK_LEVEL_LOW" }

  enabled_controls = .control_statuses.where {
    .enabled == true && is_null(.deleted_at)
  }

  no_evidence_controls = enabled_controls.where {
    .status == "CONTROL_STATUS_NO_EVIDENCE"
  }
  gap_controls = enabled_controls.where {
    .status == "CONTROL_STATUS_GAP"
  }
  validated_controls = enabled_controls.where {
    .status == "CONTROL_STATUS_VALIDATED"
  }

  active_engagements = .engagements.where { is_null(.deleted_at) }
  active_assessments = .ongoing_assessments.where {
    .active == true && is_null(.org_assessment)
  }

  business_owner = .business_owner
  external_sources = .external_sources
  solutions = .org.solutions.where { .company.id == company.id }
  assets = .assets
  tag_links = .tag_links

  subprocessor_claims = .claims.where {
    .source in subprocessorSources
    && .inherent_risk_category.category == "Data"
    && .inherent_risk_category.sub_category == "Sub-processor"
    && .inherent_risk_category.display_name == "Yes"
  }

  direct_irq_claims = .claims.where {
    .source == "irq"
    && (.inherent_risk_category.enabled == true || is_null(.inherent_risk_category.enabled))
  }

  engagement_irq_claims = active_engagements.claims.where {
    .source == "irq"
    && (.inherent_risk_category.enabled == true || is_null(.inherent_risk_category.enabled))
  }

  all_irq_claims = direct_irq_claims + engagement_irq_claims

  current_risk =
    is_null(.risk)
      ? (
          .calculated_risk == "RISK_LEVEL_UNSPECIFIED"
            ? "RISK_LEVEL_LOW"
            : .calculated_risk
        )
      : .risk

  latest_ir_change = .changes
    .where { exists(.new_risk) || exists(.new_calculated_risk) }
    .sort { .created_at.desc }
    .limit(1)
    .first()

  latest_due_date = .re_assessment_due_dates
    .where { is_null(.deleted_at) }
    .sort { .created_at.desc }
    .limit(1)
    .first()

  select {
    id,
    status,
    lifecycle,
    created_at,
    updated_at,
    assessment_score,

    risk: current_risk,
    validated_risk: .risk,
    calculated_risk: .calculated_risk,
    engagement_mode,
    hidden_in_discovery,
    risk_monitored,

    company: company.select {
      id,
      name: coalesce(.display_name, .name),
      website,
      short_description,
      long_description,
      llm_long_description,
      paragraph_description,
      llm_paragraph_description,
      founding_year,
      total_funding,
      industry_sector,
      linkedin_url,
      company_size,
      ticker
    },

    name: coalesce(.name, company.display_name, company.name),
    website: coalesce(.website, company.website),

    business_owners: business_owner.select { name },
    business_owner_emails: business_owner.select { email },

    external_sources: external_sources.select { platform },
    solutions_platforms: solutions.select { .external_source.platform },
    tag_ids: tag_links.select { .tag.id },
    projects: active_assessments.select { .project.id },

    re_assessment_due_date: latest_due_date.due_date,
    re_assessment_due_date_update_type: latest_due_date.update_type,
    re_assessment_due_date_user_id: latest_due_date.changed_by_user_id,

    inherent_risk_updated_by: latest_ir_change.actor,
    inherent_risk_updated_at: latest_ir_change.created_at,

    critical_findings: critical_signals.count(),
    org_critical_findings: org_critical_signals.count(),
    high_findings: high_signals.count(),
    org_high_findings: org_high_signals.count(),
    medium_findings: medium_signals.count(),
    org_medium_findings: org_medium_signals.count(),
    low_findings: low_signals.count(),
    org_low_findings: org_low_signals.count(),

    critical_risk_insights: critical_risk_insights.count(),
    high_risk_insights: high_risk_insights.count(),
    medium_risk_insights: medium_risk_insights.count(),
    low_risk_insights: low_risk_insights.count(),

    no_evidence_controls: no_evidence_controls.count(),
    gap_controls: gap_controls.count(),
    validated_controls: validated_controls.count(),

    is_subprocessor: subprocessor_claims.exists(),

    total_questionnaires: questionnaires.count(),
    ongoing_questionnaires: ongoing_questionnaires.count(),

    country_codes: assets.select { .company.country_code },
    engagement_ids: active_engagements.select { id },
    engagement_names: active_engagements.select { name },

    custom_fields: .custom_fields,
    inherent_risk_categories_by_category: all_irq_claims.group_by {
      key: is_null(.inherent_risk_category.sub_category)
        ? .inherent_risk_category.category
        : .inherent_risk_category.category + " - " + .inherent_risk_category.sub_category
    }.select {
      key,
      values: .items.select { .inherent_risk_category.display_name }
    },

    parent_id: parent_company.id,
    parent_name: coalesce(parent_company.name, parent_company.company.name)
  }
}
```

## 4. Why this is more Owl-like than SQL

The SQL query is forced into global staging.

The Owl version is local and scoped:

- each `org_company` is the current root
- local names like `visible_signals`, `enabled_controls`, `active_engagements`, `current_risk` are defined once
- the final `select` reads like an enriched object, not a row assembled from many joins

This is the key difference:

### SQL style
- define CTEs globally
- aggregate in separate stages
- rejoin everything later

### Owl style
- derive local facts inside the entity scope
- reuse them directly
- shape the final object in one place

That is much closer to the principle of least effort.

## 5. Where Owl is genuinely better here

### A. Entity-centered reading
The final result is a company listing. Owl starts there directly.

### B. Local derivation instead of global CTE towers
The SQL needs many CTEs because each derived dataset must be staged and then joined back.

Owl can keep most of that logic close to the entity:

```oql
org_companies {
  visible_signals = .signals.where { ... }
  critical_signals = visible_signals.where { ... }
  select { critical_findings: critical_signals.count() }
}
```

### C. Reverse navigation matters
If the schema is modeled with refs, then many SQL joins simply vanish into navigation.

Instead of:
- `join companies on org_companies.company = companies.id`

we want:
- `.company`

Instead of:
- `left join org_company_engagements on ...`

we want:
- `.engagements`

### D. Arrays and repeated data stay in one collection model
Things like:
- `engagement_names`
- `country_codes`
- `external_sources`
- `business_owners`

are all collection-valued in Owl, not special cases requiring a separate array pipeline.

## 6. Where pure Owl still needs help

This is the honest part.

The pure Owl shape above is the right direction, but today it still wants a few language-level capabilities to feel truly clean.

### A. Scoped local bindings are essential
This query is only readable in Owl if scoped blocks are real and first-class.

Without scoped blocks, the repeated derivations become noisy.

### B. Better object/list projection ergonomics
Some places want cleaner object-valued and collection-valued projection rules, for example:

```oql
business_owner.select { name }
assets.select { .company.country_code }
```

This is consistent with Owl, but it needs to be explicitly part of the language description.

### C. Distinct/dedup semantics
The SQL uses:
- `array_agg(distinct ...)`
- `array_remove(...)`
- grouped dedup patterns

Pure Owl needs a simpler native answer.

Likely direction:

```oql
external_sources.select { platform }.distinct()
```

or

```oql
external_sources.distinct { platform }
```

Without a clean distinct form, these reporting queries still feel more awkward than they should.

### D. Scoped grouping into shaped objects
The SQL builds:
- `custom_fields` as a JSON object
- `inherent_risk_categories_by_category` as a grouped object map

Pure Owl needs a comfortable way to say:
- group these values
- then shape them into an object or map

This is one of the biggest remaining gaps for complex analytics/reporting queries.

### E. Time/date derivation ergonomics
The SQL has nontrivial due-date logic and date formatting.

Pure Owl needs a clean time chapter for:
- date formatting
- duration math
- interval addition
- latest-value-by-sort patterns

### F. Collection merge/union semantics
This part:

```oql
all_irq_claims = direct_irq_claims + engagement_irq_claims
```

is natural in spirit, but Owl needs an explicit collection merge story.

## 7. Difference with SQL in one sentence

SQL asks:
- how do I stage and join all the intermediate tables?

Pure Owl asks:
- what is an `org_company`, and what derived facts do I want to name while shaping it?

That is the real difference.

## 8. What a more mature Owl version would ideally look like

If Owl had the remaining reporting ergonomics tightened, the query could feel even more direct:

```oql
org_companies {
  where { .org.id == orgId }
  where {
    discovery
      ? .lifecycle == "COMPANY_LIFECYCLE_DISCOVERY"
      : .lifecycle != "COMPANY_LIFECYCLE_DISCOVERY"
  }

  company = .company
  signals = .signals.open.visible.real_findings()
  controls = .control_statuses.enabled.active()
  questionnaires = .ongoing_questionnaires.assessment_active()
  engagements = .engagements.active()

  select {
    id,
    name: coalesce(.name, company.display_name, company.name),
    risk: current_risk(),
    findings: signals.by_risk_level(),
    controls: controls.by_status(),
    questionnaires: questionnaires.summary(),
    engagements: engagements.summary(),
    custom_fields: .custom_fields.compact(),
    inherent_risk_categories_by_category: irq_claims().grouped_categories(),
    parent: .parent.select { id, name: coalesce(.name, .company.name) }
  }
}
```

That version is not about adding magic. It is about Owl gaining a few clear reusable collection and grouping forms.

## 9. Bottom line

Yes, this can be expressed in pure Owl.

But not by copying the SQL shape.

The pure-Owl way is:
- root the query in `org_companies`
- use a scoped block
- define local derived collections and summaries once
- use navigation instead of join ceremony
- shape the final enriched object in one `select`

That is the correct Owl direction.

The main things still needed for this class of serious query are:
- scoped blocks
- distinct/dedup
- grouped object shaping
- better time semantics
- explicit collection merge/union semantics

Those are the syntax/design gaps worth closing if Owl wants to feel genuinely mature on complex reporting queries like this one.
