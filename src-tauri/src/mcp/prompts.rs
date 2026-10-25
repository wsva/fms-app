//! MCP *prompts* — the goose recipes, published without goose.
//!
//! `docs/design/agent/goose_recipe/*.yaml` already has MCP's prompt shape
//! (`title`, `description`, `instructions`, `parameters`), so serving them here
//! is a translation, not a port: any MCP client gets the same idioms with no
//! goose envelope and no per-product slash commands.
//!
//! The YAML files stay the source of truth — they are embedded with
//! `include_str!` and parsed on demand, so editing a recipe edits the prompt it
//! serves. A recipe that fails to parse is dropped from the list rather than
//! taking the whole set down.

use rmcp::model::{
    ErrorData as McpError, GetPromptResponse, GetPromptResult, ListPromptsResult, Prompt,
    PromptArgument, PromptMessage, Role,
};
use serde::Deserialize;
use serde_json::{Map, Value};

/// Every embedded recipe, by the name it is served under. The name is the file
/// stem, so `get_prompt { name }` matches what a reader of the repo would guess.
const RECIPES: [(&str, &str); 6] = [
    (
        "build-dataset-from-media",
        include_str!("../../../docs/design/agent/goose_recipe/build-dataset-from-media.yaml"),
    ),
    (
        "build-dictation-dataset",
        include_str!("../../../docs/design/agent/goose_recipe/build-dictation-dataset.yaml"),
    ),
    (
        "correct-timestamps",
        include_str!("../../../docs/design/agent/goose_recipe/correct-timestamps.yaml"),
    ),
    (
        "correct-timestamps-single",
        include_str!("../../../docs/design/agent/goose_recipe/correct-timestamps-single.yaml"),
    ),
    (
        "normalize-read-aloud-texts",
        include_str!("../../../docs/design/agent/goose_recipe/normalize-read-aloud-texts.yaml"),
    ),
    (
        "build-read-aloud-from-ebook",
        include_str!("../../../docs/design/agent/goose_recipe/build-read-aloud-from-ebook.yaml"),
    ),
];

/// The parts of a recipe this server uses. `parameters` is kept as raw values
/// because its shape belongs to goose — reading `name`/`description`/`required`
/// off each entry defensively means a recipe field this build does not model
/// cannot fail the whole parse.
#[derive(Deserialize, Default)]
struct Recipe {
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    instructions: String,
    #[serde(default)]
    parameters: Vec<Value>,
}

fn parse(yaml: &str) -> Result<Recipe, String> {
    serde_yaml::from_str::<Recipe>(yaml).map_err(|e| e.to_string())
}

/// The prompt names this build serves, for `capabilities.mcp_prompts`. Names
/// only: a client that wants the text calls `get_prompt`.
pub(crate) fn names() -> Vec<String> {
    RECIPES.iter().map(|(name, _)| (*name).to_string()).collect()
}

pub(crate) fn list() -> ListPromptsResult {
    let prompts = RECIPES
        .iter()
        .filter_map(|(name, yaml)| parse(yaml).ok().map(|recipe| (*name, recipe)))
        .map(|(name, recipe)| {
            let title = if recipe.title.is_empty() {
                name.to_string()
            } else {
                recipe.title.clone()
            };
            Prompt::new(
                name,
                Some(recipe.description.clone()),
                arguments_of(&recipe),
            )
            .with_title(title)
        })
        .collect();

    ListPromptsResult::with_all_items(prompts)
}

/// Render one prompt. `arguments` is optional because no recipe in this build
/// declares any; a client that passes them anyway gets them substituted into
/// `{{name}}` placeholders, and appended as a parameter block when no
/// placeholder exists — silently dropping a caller's input would be worse.
pub(crate) fn get(
    name: &str,
    arguments: Option<&Map<String, Value>>,
) -> Result<GetPromptResponse, McpError> {
    let (_, yaml) = RECIPES
        .iter()
        .find(|(n, _)| *n == name)
        .ok_or_else(|| unknown_prompt(name))?;

    let recipe = parse(yaml).map_err(|e| {
        McpError::internal_error(
            format!(
                "the embedded recipe '{}' is not valid YAML ({}). This is a build defect: the \
                 prompt is compiled in from docs/design/agent/goose_recipe/{}.yaml.",
                name, e, name
            ),
            None,
        )
    })?;

    let mut body = if recipe.instructions.trim().is_empty() {
        recipe.description.clone()
    } else {
        recipe.instructions.trim().to_string()
    };

    if let Some(args) = arguments.filter(|a| !a.is_empty()) {
        let mut leftover: Vec<(String, String)> = Vec::new();
        for (key, value) in args {
            let placeholder = format!("{{{{{}}}}}", key);
            let rendered = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if body.contains(&placeholder) {
                body = body.replace(&placeholder, &rendered);
            } else {
                leftover.push((key.clone(), rendered));
            }
        }
        if !leftover.is_empty() {
            body.push_str("\n\n## Parameters\n\n");
            for (key, value) in leftover {
                body.push_str(&format!("- **{}**: {}\n", key, value));
            }
        }
    }

    let result = GetPromptResult::new(vec![PromptMessage::new_text(Role::User, body)])
        .with_description(if recipe.description.is_empty() {
            recipe.title
        } else {
            recipe.description
        });
    Ok(result.into())
}

fn unknown_prompt(name: &str) -> McpError {
    McpError::invalid_params(
        format!(
            "unknown prompt '{}'. Available prompts: {}. Call prompts/list for their \
             descriptions, then get_prompt with one of these names.",
            name,
            names().join(", ")
        ),
        None,
    )
}

/// The declared arguments, or `None` when the recipe takes none — an empty
/// `arguments` array is a different (and noisier) thing than no arguments.
fn arguments_of(recipe: &Recipe) -> Option<Vec<PromptArgument>> {
    let args: Vec<PromptArgument> = recipe
        .parameters
        .iter()
        .filter_map(|p| {
            let name = p.get("name")?.as_str()?.to_string();
            let description = p
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let required = p.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
            Some(
                PromptArgument::new(name)
                    .with_description(description)
                    .with_required(required),
            )
        })
        .collect();

    if args.is_empty() {
        None
    } else {
        Some(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_recipe_parses_and_has_instructions() {
        for (name, yaml) in RECIPES {
            let recipe = parse(yaml).unwrap_or_else(|e| panic!("recipe {name} does not parse: {e}"));
            assert!(
                !recipe.instructions.trim().is_empty(),
                "recipe {name} has no instructions"
            );
            assert!(
                !recipe.description.trim().is_empty(),
                "recipe {name} has no description"
            );
        }
    }

    #[test]
    fn served_names_match_the_embedded_set() {
        assert_eq!(names().len(), RECIPES.len());
        for (name, _) in RECIPES {
            assert!(names().contains(&name.to_string()));
        }
    }

    #[test]
    fn unknown_prompt_names_the_valid_set() {
        let err = get("not-a-recipe", None).unwrap_err();
        let text = format!("{:?}", err);
        assert!(text.contains("build-dictation-dataset"), "error was: {text}");
    }
}
