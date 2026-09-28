# Two systems, two motivations

## XP — Progress, Non-spendable
- Earned through learning and achievements.
- Accumulates permanently.
- Determines levels, milestones, titles, and profile progress.
- Cannot be bought, sold, gifted, or lost.
- 
## Coin — Purchasing power, Spendable
- Earned through learning activities and achievements.
- Spent on datasets.
- Can potentially be gifted or traded in a later version.
- Can be controlled through issuance limits and sinks.

## No real-money trading

# XP System — Learning Progression

## 1. Core Design Principles
XP represents a user's permanent learning progress. Users earn XP through meaningful learning activities, mastery, retention, achievements, and verified community contributions.

XP is:
- **Non-spendable:** XP cannot be used to purchase datasets.
- **Non-transferable:** Users cannot buy, sell, or gift XP.
- **Permanent:** Earned XP is never removed.
- **Non-purchasable:** Real money cannot be used to buy XP.
- **Independent of Coin:** Coin is a separate currency used to purchase datasets.

The central principle is:

**XP measures learning progress and achievement, not wealth or purchasing power.**

## 2. XP and Coin

| Property | XP | Coin |
|---|---|---|
| Purpose | Learning progression | Purchasing datasets |
| Earned through | Learning and achievements | Learning and achievements |
| Spendable | No | Yes |
| Transferable | No | Optional future feature |
| Purchasable with real money | No | Not initially |
| Determines level | Yes | No |
| Used in dataset purchases | No | Yes |

XP and Coin can be earned from the same learning activity, but they serve different purposes.

XP provides long-term progression, while Coin provides a practical reason to participate in the dataset marketplace.

## 3. How Users Earn XP

XP has four primary sources.

### 3.1 Practice XP

Reward users for completing meaningful learning activities.

| Activity | Base XP |
|---|---:|
| Complete a focused 5-minute session | 5 |
| Complete a focused 10-minute session | 10 |
| Complete a focused 20-minute session | 20 |
| Complete a planned learning session | +5 |

Qualifying activities may include:
- Shadowing practice
- Vocabulary review
- Listening comprehension
- Subtitle-based learning
- Completing a structured lesson

Rewards should depend on meaningful participation, not simply keeping a video or audio player running.

### 3.2 Mastery XP

Reward users for demonstrating that they have learned something.

| Achievement | Example XP |
|---|---:|
| Correctly recall a new word | 2 |
| Correctly recall a difficult word | 3 |
| Complete a vocabulary review session | 5 |
| Demonstrate mastery of a vocabulary set | 20 |

Mastery rewards should be awarded according to defined learning milestones.

For example:
- Initial successful recall earns a small reward.
- Successful recall on a later day earns a retention bonus.
- Repeated successful recalls eventually mark the word as mastered.
- Once mastered, routine reviews no longer repeatedly generate mastery XP.

### 3.3 Retention XP

Reward users for remembering material after a meaningful interval.

| Retention interval | Example bonus |
|---|---:|
| 1 day | 2 XP |
| 7 days | 5 XP |
| 30 days | 10 XP |

Each retention milestone should be awarded only once per defined learning objective.

This encourages users to revisit material instead of constantly chasing new content.

### 3.4 Achievement XP

Achievements provide memorable rewards for reaching significant milestones.

| Achievement | Condition | XP |
|---|---|---:|
| First Steps | Complete the first learning session | 25 |
| Word Collector | Learn 100 distinct vocabulary items | 100 |
| Shadowing Apprentice | Complete 50 qualifying shadowing exercises | 150 |
| Consistent Learner | Complete 30 learning sessions on separate days | 200 |
| Vocabulary Master | Demonstrate mastery of 500 words | 500 |
| Dedicated Learner | Complete 100 qualifying learning sessions | 300 |

These values are initial balancing examples.

Most achievements should be permanent and awarded once. Repeatable achievements should have explicit limits or diminishing rewards.

## 4. Level Progression

Use a quadratic XP curve to make early progression accessible while allowing long-term growth.

The total XP required to reach level L is:

`XP_required(L) = 100 × (L - 1)²`

| Level | Total XP Required | XP Required for Next Level |
|---:|---:|---:|
| 1 | 0 | 100 |
| 2 | 100 | 300 |
| 3 | 400 | 500 |
| 4 | 900 | 700 |
| 5 | 1,600 | 900 |
| 10 | 8,100 | 1,900 |
| 20 | 36,100 | 3,900 |
| 50 | 240,100 | 9,900 |

The formula is:

`Level = floor(sqrt(LifetimeXP / 100)) + 1`

Level should always be derived from Lifetime XP rather than stored as an independently editable value.

The curve should be calibrated against real user behavior. The objective is to make early progress rewarding without making higher levels trivial.

## 6. Preventing XP Farming

XP should reward genuine learning rather than repetitive activity.

### Rule 1: Reward unique progress

A mastery milestone should normally award XP only once.

### Rule 2: Reward meaningful practice

Require evidence of a completed learning activity, such as a vocabulary recall, shadowing attempt, or comprehension exercise.

### Rule 3: Diminish repetitive rewards

Repeated practice can continue to earn XP, but repeatedly performing the same easy task should not be the most efficient way to level up.

### Rule 4: Reward retention

Long-term recall should be more valuable than repeatedly reviewing the same material within a short period.

### Rule 5: Keep achievements auditable

Every achievement should have a defined condition, a clear reward, and a record of when it was unlocked.

### Rule 6: Avoid harsh daily caps

Users should be able to continue learning for as long as they want.

Instead of preventing further practice, reduce or eliminate repetitive bonus rewards when appropriate.

## 7. XP for Dataset Contributions

Dataset creation and improvement can earn contribution XP, but rewards should reflect quality rather than quantity.

| Contribution | Reward Policy |
|---|---|
| Upload a dataset | No automatic reward solely for uploading |
| Pass a quality review | Award contribution XP |
| Correct a verified subtitle error | Award a small reward |
| Improve an existing dataset | Reward verified improvements |
| Reach a contribution milestone | Award a one-time achievement |
| Sell a dataset | No XP based solely on sales revenue |

Dataset contribution rewards should consider:
- Completeness
- Accuracy
- Originality
- Reusability
- Verified corrections
- Meaningful independent usage

Avoid rewarding raw download counts or unverified ratings, because these can be manipulated.

Contribution XP should be separate from Coin earned through dataset sales.

## 8. Preventing XP Inflation

Since XP cannot be spent, inflation is primarily a progression-design problem.

The goal is to preserve the meaning of levels and achievements as the user base grows.

Use these mechanisms:

1. Establish predictable XP rewards.
2. Use increasing level thresholds.
3. Award one-time mastery and achievement bonuses.
4. Reduce rewards for repetitive activities.
5. Prevent duplicate reward events.
6. Monitor XP earned per active user.
7. Adjust future reward rules when necessary.

Do not remove XP that users have already earned simply because the economy or progression curve changes.

If the level curve changes, existing Lifetime XP should remain intact. The change should affect future progression transparently.

Track:
- Median Lifetime XP per active user
- XP earned per active user per week
- Time required to reach each level
- Achievement completion rates
- Percentage of XP from practice, mastery, retention, and contributions

The most important question is whether a level still represents a meaningful amount of learning.

# Coin System Design

## 1. Core Concept

Coin is a spendable currency used to activate datasets, earn through learning, and participate in team activities.

The system has four main mechanics:

1. **Spend Coin** to start a new dataset.
2. **Earn Coin back** by making progress through a dataset.
3. **Refund Coin** when removing an incomplete dataset, with a deduction.
4. **Earn bonus Coin** through team learning.

The goal is to make Coin a learning-funded currency that encourages users to start, continue, and complete meaningful learning activities.

## 2. Spending Coin to Start a Dataset

Charge a one-time activation fee when a user adds a dataset to their active learning library.

| Dataset Type | Suggested Activation Cost |
|---|---:|
| Small | 10 Coin |
| Medium | 25 Coin |
| Large | 50 Coin |
| Premium / Curated | 75–100 Coin |

These are initial balancing values. The final cost should depend on dataset size, quality, and expected learning time.

### Dataset Ownership vs. Activation

Consider separating dataset acquisition from activation:

- **Acquire:** The user purchases or gains access to a dataset.
- **Activate:** The user spends Coin to begin a learning journey with it.

This allows users to own multiple datasets but activate only a few at a time.

For a simpler MVP, skip the ownership distinction and charge Coin only when users add datasets to their active learning library.

A free starter dataset is also recommended so new users can experience the learning loop before they need to earn Coin.

## 3. Earning Coin Through Dataset Progress

Users should earn Coin through meaningful learning milestones rather than simply spending time in a dataset.

For example, a dataset with a 50-Coin activation fee could return the full amount as the user progresses.

| Completion Milestone | Coin Reward | Total Returned |
|---|---:|---:|
| 10% | 5 | 5 |
| 25% | 5 | 10 |
| 50% | 10 | 20 |
| 75% | 10 | 30 |
| 100% | 20 | 50 |
| **Total** | **50** | **50** |

This is an initial model: users can recover the full activation cost by completing the dataset.

### What Counts as Progress?

For FMS, qualifying activities could include:

- Completing a meaningful group of subtitle cues.
- Finishing a shadowing exercise.
- Correctly recalling vocabulary.
- Reviewing previously learned material after a meaningful interval.
- Completing a dataset milestone.

Avoid awarding Coin merely for opening a dataset, playing media, or repeatedly marking the same item as completed.

### Mastery Bonus

To encourage long-term learning, add a small bonus for demonstrated retention.

For example:

- Complete the dataset: recover the activation cost.
- Pass a delayed retention review: earn an additional 5–10 Coin.

This rewards both completing content and remembering what was learned.

## 4. Refunds for Removing an Incomplete Dataset

When a user removes an incomplete dataset, refund part of the original activation cost according to the completion percentage.

| Completion at Removal | Refund |
|---|---:|
| 0–10% | 80% |
| 11–25% | 60% |
| 26–50% | 40% |
| 51–75% | 20% |
| 76–99% | 0% |
| 100% | Not applicable |

For example, if a dataset costs 50 Coin to activate and the user removes it at 30% completion, they receive 20 Coin back.

### Refund Rules

- Refund only the original activation payment.
- Do not refund Coin already earned through learning.
- Allow only one refund per activation.
- Re-activating a dataset must not reset its reward milestones.
- Consider a 24-hour grace period for a full refund if the user has barely started.
- If a dataset becomes unavailable or is removed by an administrator, consider a full refund.

### Preventing Refund Exploits

Users should not be able to repeatedly activate and abandon datasets to farm rewards.

Tie reward eligibility to persistent learning history rather than the current activation. Once a milestone has paid out, removing and re-adding the dataset must not make that milestone eligible again.

## 5. Extra Coin Through Team Learning

Team rewards should be a bonus on top of individual learning rewards.

For example, each member of a four-person team completes a qualifying learning session:

| Reward | Coin |
|---|---:|
| Individual learning reward | 5 |
| Team completion bonus | +2 |
| **Total per member** | **7** |

### Team Reward Ideas

| Activity | Suggested Bonus |
|---|---:|
| Complete a shared daily goal | +2 Coin |
| All team members complete a weekly goal | +5 Coin |
| Complete a team learning challenge | +5–15 Coin |
| Maintain a team learning streak | Small periodic bonus |

### Team Reward Requirements

- Each member must complete a minimum amount of meaningful learning.
- The same learning event cannot earn the same team bonus repeatedly.
- Team rewards should have daily or weekly limits.
- Rewards are granted only after the shared goal is verified.
- Creating multiple accounts must not multiply rewards.

Team bonuses should reward genuine collaboration, not merely joining a team or inviting friends.

## 6. Coin Economy and Inflation

A critical decision is what happens to Coin spent on dataset activation.

### Model A: Coin Sink

Activation payments are removed from circulation.

- Users spend Coin to activate datasets.
- Learning activities create new Coin.
- Activation fees help control inflation.

This is the simplest model if datasets are provided by the platform.

### Model B: Marketplace Transfer

Activation payments go to dataset creators, potentially minus a platform fee.

- Users spend Coin.
- Creators receive Coin.
- The platform may collect a fee.

Transfers do not reduce the total Coin supply unless some of the payment is removed from circulation.

If FMS has a dataset marketplace, consider separating the dataset purchase price from the activation fee. This prevents the learning-reward economy from becoming entangled with creator revenue.

### Metrics to Monitor

- Coin issued per active user per week.
- Coin spent on activation and other features.
- Average user Coin balance.
- Dataset activation and abandonment rates.
- Dataset completion and retention rates.
- Team bonus issuance.

If Coin balances rise too quickly, adjust future reward rates or introduce useful Coin sinks. Avoid arbitrarily removing existing user balances.

## 7. Recommended MVP Rules

| Feature | Initial Rule |
|---|---|
| Activation | One-time fee based on dataset size |
| Progress rewards | Earn back up to 100% of the activation fee |
| Mastery | Small bonus for verified delayed recall |
| Refund | 80%, 60%, 40%, 20%, or 0%, based on completion |
| Team learning | Modest bonus for verified shared goals |
| Abuse prevention | Unique learning events and server-side validation |
| Expiration | Coin never expires |
| XP relationship | Coin rewards do not affect Lifetime XP |

## 8. PostgreSQL Implementation

Keep Coin separate from XP, but use the same ledger-based architecture.

### Suggested Tables

- `user_coin`
  - Cached current Coin balance.

- `coin_ledger`
  - Append-only record of every earning, spending, refund, and adjustment.

- `coin_reward_rule`
  - Versioned rules for progress and team rewards.

- `dataset_activation`
  - User, dataset, activation cost, progress, status, and refund state.

- `dataset_reward_claim`
  - Records which milestones have already paid out.

- `team_reward_claim`
  - Records which shared goals have already paid out.

### Transaction and Security Rules

- Process every reward, payment, and refund in a database transaction.
- Use unique constraints to prevent duplicate milestone rewards.
- Make reward claims idempotent so retries cannot award Coin twice.
- Calculate reward eligibility on the server.
- Never trust progress or reward amounts supplied by the client.
- Keep the ledger append-only; correct mistakes with reversal entries.

## 9. Final Recommendation

Use Coin as a **recoverable learning deposit**:

1. Users pay Coin to activate a dataset.
2. Completing the dataset returns the activation cost.
3. Demonstrated retention and team learning generate extra Coin.
4. Abandoning a dataset early returns only part of the activation cost.
5. Coin never expires because of inactivity.
6. XP remains separate and represents permanent learning progress.

This gives Coin a clear purpose: encouraging users to start, continue, and complete meaningful learning activities.
