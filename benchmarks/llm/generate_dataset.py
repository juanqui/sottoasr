#!/usr/bin/env python3
"""Generate a comprehensive benchmark dataset for transcript cleanup evaluation.

Creates a CSV with 100+ samples across multiple categories:
  - Filler word removal
  - Crutch word removal
  - Self-correction handling
  - False start removal
  - Grammar correction
  - List formatting
  - Misheard/technical word correction
  - Mixed complexity
  - Short utterances
  - Long dictation

Each sample has: id, category, raw transcript, expected clean output, and notes.
"""

import csv
from pathlib import Path

OUTPUT = Path(__file__).parent / "dataset.csv"

SAMPLES = [
    # =========================================================================
    # CATEGORY: filler_removal (15 samples)
    # =========================================================================
    ("filler_01", "filler_removal", "I uh need you to uh send me the report", "I need you to send me the report.", "Simple uh removal"),
    ("filler_02", "filler_removal", "Um can we schedule a meeting for um Thursday", "Can we schedule a meeting for Thursday?", "Um at start and middle"),
    ("filler_03", "filler_removal", "The uh database uh connection is uh timing out", "The database connection is timing out.", "Multiple uh in technical context"),
    ("filler_04", "filler_removal", "So like I was thinking we could like maybe try a different approach", "I was thinking we could try a different approach.", "Like as filler"),
    ("filler_05", "filler_removal", "You know the thing is you know we need more time", "The thing is we need more time.", "You know as filler"),
    ("filler_06", "filler_removal", "I think uh we should um probably uh consider uh using a cache", "I think we should probably consider using a cache.", "Heavy filler density"),
    ("filler_07", "filler_removal", "Uh yeah uh I'll uh get that done uh by Friday", "Yeah, I'll get that done by Friday.", "Filler at boundaries"),
    ("filler_08", "filler_removal", "The um the performance uh metrics look um pretty good", "The performance metrics look pretty good.", "Fillers around articles"),
    ("filler_09", "filler_removal", "Can you uh check if the um server is uh responding", "Can you check if the server is responding?", "Question with fillers"),
    ("filler_10", "filler_removal", "I uh I think we should uh go with option B", "I think we should go with option B.", "Filler with stutter"),
    ("filler_11", "filler_removal", "So uh the thing is um we don't have uh enough uh resources", "The thing is we don't have enough resources.", "So as filler + others"),
    ("filler_12", "filler_removal", "Uh hello uh my name is uh John and I'm uh calling about um my account", "Hello, my name is John and I'm calling about my account.", "Introduction with fillers"),
    ("filler_13", "filler_removal", "The uh the new uh feature should um be ready by uh next week", "The new feature should be ready by next week.", "Repeated article + fillers"),
    ("filler_14", "filler_removal", "We need to um talk about uh the budget uh for Q3", "We need to talk about the budget for Q3.", "Business context"),
    ("filler_15", "filler_removal", "Uh just uh wanted to uh let you know that uh everything's fine", "Just wanted to let you know that everything's fine.", "Casual message heavy fillers"),

    # =========================================================================
    # CATEGORY: crutch_words (10 samples)
    # =========================================================================
    ("crutch_01", "crutch_words", "So basically what we need to do is refactor the auth module", "What we need to do is refactor the auth module.", "So basically"),
    ("crutch_02", "crutch_words", "I mean honestly I think the design is pretty solid", "I think the design is pretty solid.", "I mean honestly"),
    ("crutch_03", "crutch_words", "Basically the whole system is basically built on microservices", "The whole system is built on microservices.", "Double basically"),
    ("crutch_04", "crutch_words", "You know what I mean like the API is just not performant enough", "The API is just not performant enough.", "You know what I mean like"),
    ("crutch_05", "crutch_words", "So yeah anyway the point is we need more tests", "The point is we need more tests.", "So yeah anyway"),
    ("crutch_06", "crutch_words", "At the end of the day it's really about user experience right", "At the end of the day, it's really about user experience.", "Right as seeking agreement"),
    ("crutch_07", "crutch_words", "Like literally the entire codebase needs to be reviewed", "The entire codebase needs to be reviewed.", "Like literally"),
    ("crutch_08", "crutch_words", "Okay so the thing is basically we're running out of disk space", "We're running out of disk space.", "Okay so + basically"),
    ("crutch_09", "crutch_words", "I guess what I'm trying to say is that the tests are failing", "The tests are failing.", "I guess what I'm trying to say"),
    ("crutch_10", "crutch_words", "So like at this point honestly we should just rewrite it", "At this point, we should just rewrite it.", "Multiple crutch words"),

    # =========================================================================
    # CATEGORY: self_correction (15 samples)
    # =========================================================================
    ("selfcorr_01", "self_correction", "Send the email to marketing, wait, actually send it to engineering", "Send the email to engineering.", "Simple correction with wait actually"),
    ("selfcorr_02", "self_correction", "The deadline is Friday, no, Monday, we have until Monday", "The deadline is Monday.", "Correction with no"),
    ("selfcorr_03", "self_correction", "Use Python, actually no, let's use Rust for this", "Let's use Rust for this.", "Actually no"),
    ("selfcorr_04", "self_correction", "Set the font size to twelve, no wait, fourteen pixels", "Set the font size to fourteen pixels.", "No wait"),
    ("selfcorr_05", "self_correction", "Deploy to the dev environment, actually let's go straight to staging", "Deploy to the staging environment.", "Actually + rephrase"),
    ("selfcorr_06", "self_correction", "The meeting is at two, sorry, two thirty PM", "The meeting is at two thirty PM.", "Sorry as correction marker"),
    ("selfcorr_07", "self_correction", "We should use Redis, wait no, Memcached would be better for this use case", "We should use Memcached for this use case.", "Wait no with reasoning"),
    ("selfcorr_08", "self_correction", "Add a loading spinner, actually scratch that, add a skeleton screen instead", "Add a skeleton screen.", "Scratch that"),
    ("selfcorr_09", "self_correction", "The buffer size should be 1024, no, 2048, actually 4096 bytes", "The buffer size should be 4096 bytes.", "Triple correction"),
    ("selfcorr_10", "self_correction", "I'll handle the frontend, actually let Sarah take the frontend, I'll do the backend", "Let Sarah take the frontend, I'll do the backend.", "Task reassignment correction"),
    ("selfcorr_11", "self_correction", "Let's schedule it for next Tuesday, oh wait, that's a holiday, make it Wednesday", "Let's schedule it for next Wednesday.", "Correction with reason"),
    ("selfcorr_12", "self_correction", "The max retries should be three, no five, we need five retries for reliability", "The max retries should be five for reliability.", "Correction with justification"),
    ("selfcorr_13", "self_correction", "Run the tests on CI, actually just run them locally first", "Run the tests locally first.", "Actually as mid-sentence correction"),
    ("selfcorr_14", "self_correction", "Import it from the utils module, no, the helpers module", "Import it from the helpers module.", "Simple swap correction"),
    ("selfcorr_15", "self_correction", "The API rate limit should be 100 per minute, wait, that's too low, make it 500 per minute", "The API rate limit should be 500 per minute.", "Correction with evaluation"),

    # =========================================================================
    # CATEGORY: false_start (10 samples)
    # =========================================================================
    ("falsestart_01", "false_start", "The the server needs to be restarted", "The server needs to be restarted.", "Simple word repetition"),
    ("falsestart_02", "false_start", "I think I think we should add more logging", "I think we should add more logging.", "Phrase repetition"),
    ("falsestart_03", "false_start", "We need to we should probably add input validation", "We should probably add input validation.", "False start then rephrase"),
    ("falsestart_04", "false_start", "The the main the primary concern is security", "The primary concern is security.", "Multiple false starts"),
    ("falsestart_05", "false_start", "Can you can you please review my pull request", "Can you please review my pull request?", "Polite request with stutter"),
    ("falsestart_06", "false_start", "What we what I wanted to say is that the tests pass", "What I wanted to say is that the tests pass.", "False start mid-thought"),
    ("falsestart_07", "false_start", "The function the method takes two parameters", "The method takes two parameters.", "Technical term restart"),
    ("falsestart_08", "false_start", "It should it needs to handle null values gracefully", "It needs to handle null values gracefully.", "Should to needs correction"),
    ("falsestart_09", "false_start", "Let's let's go ahead and merge this PR", "Let's go ahead and merge this PR.", "Let's repetition"),
    ("falsestart_10", "false_start", "The response the API response includes a timestamp", "The API response includes a timestamp.", "Noun phrase restart"),

    # =========================================================================
    # CATEGORY: grammar (10 samples)
    # =========================================================================
    ("grammar_01", "grammar", "We gonna need more time for this feature", "We're going to need more time for this feature.", "gonna → going to"),
    ("grammar_02", "grammar", "The tests is failing on the CI pipeline", "The tests are failing on the CI pipeline.", "Subject-verb agreement"),
    ("grammar_03", "grammar", "Him and me will work on the migration", "He and I will work on the migration.", "Pronoun case"),
    ("grammar_04", "grammar", "Theres three bugs that needs to be fixed", "There are three bugs that need to be fixed.", "Multiple agreement issues"),
    ("grammar_05", "grammar", "The data have been migrated successful", "The data has been migrated successfully.", "Data + adverb form"),
    ("grammar_06", "grammar", "We should of tested this before deploying", "We should have tested this before deploying.", "Should of → should have"),
    ("grammar_07", "grammar", "Me and the team is working on fixing it", "The team and I are working on fixing it.", "Multiple grammar issues"),
    ("grammar_08", "grammar", "The code dont work on production it keeps crashing", "The code doesn't work in production. It keeps crashing.", "Missing negation + run-on"),
    ("grammar_09", "grammar", "Each of the developers need to update their environment", "Each of the developers needs to update their environment.", "Each + verb agreement"),
    ("grammar_10", "grammar", "Its important that we test the edge cases its gonna save us time", "It's important that we test the edge cases. It's going to save us time.", "Its vs it's + gonna"),

    # =========================================================================
    # CATEGORY: list_formatting (10 samples)
    # =========================================================================
    ("list_01", "list_formatting", "There are three steps first clone the repo second install dependencies third run the tests", "There are three steps:\n1. Clone the repo\n2. Install dependencies\n3. Run the tests", "First/second/third pattern"),
    ("list_02", "list_formatting", "The priorities are one fix the login bug two add the search feature three update the docs", "The priorities are:\n1. Fix the login bug\n2. Add the search feature\n3. Update the docs", "One/two/three pattern"),
    ("list_03", "list_formatting", "We need to do three things one update the schema two migrate the data and three test everything", "We need to do three things:\n1. Update the schema\n2. Migrate the data\n3. Test everything", "One/two/three with connector"),
    ("list_04", "list_formatting", "The checklist includes one code review two unit tests three integration tests and four deployment verification", "The checklist includes:\n1. Code review\n2. Unit tests\n3. Integration tests\n4. Deployment verification", "Four-item list"),
    ("list_05", "list_formatting", "Todo items first refactor the auth module second add rate limiting third write documentation", "Todo items:\n1. Refactor the auth module\n2. Add rate limiting\n3. Write documentation", "Todo with first/second/third"),
    ("list_06", "list_formatting", "The requirements are one a fast API two low latency three high availability and four easy monitoring", "The requirements are:\n1. A fast API\n2. Low latency\n3. High availability\n4. Easy monitoring", "Requirements list"),
    ("list_07", "list_formatting", "Step one open the terminal step two navigate to the project directory step three run the build command", "1. Open the terminal\n2. Navigate to the project directory\n3. Run the build command", "Step-by-step instructions"),
    ("list_08", "list_formatting", "We discussed three topics first the budget second the timeline and third the team allocation", "We discussed three topics:\n1. The budget\n2. The timeline\n3. The team allocation", "Discussion topics"),
    ("list_09", "list_formatting", "The changes include one new error handling two improved logging three better test coverage four updated CI config and five revised documentation", "The changes include:\n1. New error handling\n2. Improved logging\n3. Better test coverage\n4. Updated CI config\n5. Revised documentation", "Five-item list"),
    ("list_10", "list_formatting", "Action items are one Juan to review the PR by Tuesday two Sarah to update the tests by Wednesday three Mike to deploy to staging by Thursday", "Action items:\n1. Juan to review the PR by Tuesday\n2. Sarah to update the tests by Wednesday\n3. Mike to deploy to staging by Thursday", "Action items with assignees"),

    # =========================================================================
    # CATEGORY: misheard_words (10 samples)
    # =========================================================================
    ("misheard_01", "misheard_words", "We should use oh auth two for the authentication flow", "We should use OAuth 2.0 for the authentication flow.", "oh auth → OAuth"),
    ("misheard_02", "misheard_words", "The open API spec needs to be updated", "The OpenAPI spec needs to be updated.", "open API → OpenAPI"),
    ("misheard_03", "misheard_words", "Deploy the app to the Kuber Netties cluster", "Deploy the app to the Kubernetes cluster.", "Kuber Netties → Kubernetes"),
    ("misheard_04", "misheard_words", "We're using post gress for the database", "We're using Postgres for the database.", "post gress → Postgres"),
    ("misheard_05", "misheard_words", "The Jason payload is malformed", "The JSON payload is malformed.", "Jason → JSON"),
    ("misheard_06", "misheard_words", "Add a get request to the rest API", "Add a GET request to the REST API.", "Capitalization of HTTP/API terms"),
    ("misheard_07", "misheard_words", "The container is running on docker", "The container is running on Docker.", "docker → Docker"),
    ("misheard_08", "misheard_words", "We need to update the package dot Jason file", "We need to update the package.json file.", "dot Jason → .json"),
    ("misheard_09", "misheard_words", "Set up a CI CD pipeline with git hub actions", "Set up a CI/CD pipeline with GitHub Actions.", "git hub → GitHub"),
    ("misheard_10", "misheard_words", "The web socket connection keeps dropping", "The WebSocket connection keeps dropping.", "web socket → WebSocket"),

    # =========================================================================
    # CATEGORY: mixed_complexity (15 samples)
    # =========================================================================
    ("mixed_01", "mixed", "So uh basically I think we should uh use oh auth two for the API, wait actually let's use API keys instead because they're simpler", "I think we should use API keys for the API because they're simpler.", "Fillers + crutch + self-correction + misheard"),
    ("mixed_02", "mixed", "The uh the function should um return a list of uh users, no wait, it should return a single user object with all their data", "The function should return a single user object with all their data.", "Fillers + false start + self-correction"),
    ("mixed_03", "mixed", "Okay so like I was thinking we need three things one better error handling two uh more comprehensive logging and three um automated testing", "I was thinking we need three things:\n1. Better error handling\n2. More comprehensive logging\n3. Automated testing", "Crutch words + fillers + list"),
    ("mixed_04", "mixed", "The uh post gress database is uh timing out we gonna need to uh increase the connection pool size", "The Postgres database is timing out. We're going to need to increase the connection pool size.", "Fillers + misheard + grammar"),
    ("mixed_05", "mixed", "Um we should we need to add input validation to the the API endpoint because uh users are sending malformed Jason payloads", "We need to add input validation to the API endpoint because users are sending malformed JSON payloads.", "Fillers + false start + misheard"),
    ("mixed_06", "mixed", "So basically the the bug is that uh when you click the submit button it it doesn't uh do anything right", "The bug is that when you click the submit button, it doesn't do anything.", "Crutch + false start + fillers + right"),
    ("mixed_07", "mixed", "I think we should uh deploy to staging first, actually no let's go straight to production because we already tested it on staging last week", "I think we should go straight to production because we already tested it on staging last week.", "Fillers + self-correction with reasoning"),
    ("mixed_08", "mixed", "The um the code review found uh three issues one missing error handling two uh no input validation and three basically no tests at all", "The code review found three issues:\n1. Missing error handling\n2. No input validation\n3. No tests at all", "Fillers + false start + crutch + list"),
    ("mixed_09", "mixed", "We're gonna use uh docker compose for the the local development setup and uh Kuber Netties for production", "We're going to use Docker Compose for the local development setup and Kubernetes for production.", "Grammar + fillers + false start + misheard"),
    ("mixed_10", "mixed", "Okay uh so like the thing is um we don't have enough test coverage and its causing like a lot of regressions you know", "We don't have enough test coverage and it's causing a lot of regressions.", "Multiple crutch words + fillers + grammar"),
    ("mixed_11", "mixed", "I want you to um add a loading state, no wait, add a skeleton screen to the uh dashboard page while the data is um being fetched from the API", "I want you to add a skeleton screen to the dashboard page while the data is being fetched from the API.", "Fillers + self-correction"),
    ("mixed_12", "mixed", "The the performance is uh really bad on on mobile devices we're gonna need to uh optimize the the bundle size and uh lazy load the the heavy components", "The performance is really bad on mobile devices. We're going to need to optimize the bundle size and lazy load the heavy components.", "Heavy false starts + fillers + grammar"),
    ("mixed_13", "mixed", "So first thing we need to uh fix the login bug second we should um add the search feature and third uh update the documentation right", "1. Fix the login bug\n2. Add the search feature\n3. Update the documentation", "Crutch + fillers + list + right"),
    ("mixed_14", "mixed", "Can you uh check if the uh CI CD pipeline is uh working because the the last three builds um failed and we don't know why basically", "Can you check if the CI/CD pipeline is working? The last three builds failed and we don't know why.", "Fillers + false start + misheard + crutch"),
    ("mixed_15", "mixed", "So like honestly at the end of the day we just need to uh ship the feature you know and uh stop over engineering it", "We just need to ship the feature and stop over-engineering it.", "Heavy crutch words + fillers"),

    # =========================================================================
    # CATEGORY: short_utterance (10 samples)
    # =========================================================================
    ("short_01", "short", "Uh yes", "Yes.", "Minimal with filler"),
    ("short_02", "short", "Um no I don't think so", "No, I don't think so.", "Short negative"),
    ("short_03", "short", "Let's uh merge it", "Let's merge it.", "Short command"),
    ("short_04", "short", "Ship it", "Ship it.", "Clean short — should pass through"),
    ("short_05", "short", "Looks good to me", "Looks good to me.", "LGTM — clean passthrough"),
    ("short_06", "short", "Can you uh fix that bug", "Can you fix that bug?", "Short question"),
    ("short_07", "short", "Um approved", "Approved.", "Single word with filler"),
    ("short_08", "short", "Yeah that works", "That works.", "Yeah removal"),
    ("short_09", "short", "Uh the test is uh passing now", "The test is passing now.", "Short statement"),
    ("short_10", "short", "Okay uh so basically yeah it's done", "It's done.", "Heavily padded short"),

    # =========================================================================
    # CATEGORY: long_dictation (5 samples)
    # =========================================================================
    ("long_01", "long_dictation",
     "Okay so uh I wanted to give everyone a quick update on where we stand with the project. The uh the frontend redesign is about eighty percent complete. We've uh finished the new dashboard layout and the uh user profile pages. The settings page is still uh in progress. On the backend side we uh we've completed the new API endpoints for user management and uh we're currently working on the search functionality. The main blocker right now is the uh the database migration. We need to migrate about uh two million records from the old schema to the new one and uh we're still testing the migration script to make sure it handles all the edge cases. Expected completion for the migration is uh next Wednesday. After that we should be able to uh deploy the whole thing to staging for uh QA testing.",
     "I wanted to give everyone a quick update on where we stand with the project. The frontend redesign is about 80% complete. We've finished the new dashboard layout and the user profile pages. The settings page is still in progress. On the backend side, we've completed the new API endpoints for user management and we're currently working on the search functionality. The main blocker right now is the database migration. We need to migrate about two million records from the old schema to the new one and we're still testing the migration script to make sure it handles all the edge cases. Expected completion for the migration is next Wednesday. After that, we should be able to deploy the whole thing to staging for QA testing.",
     "Project status update with many fillers"),
    ("long_02", "long_dictation",
     "So the main issue we're seeing is that the response times for the search endpoint are uh way too slow. Right now it's taking about uh three to four seconds for a simple query and um users are complaining. I did some profiling and it looks like the bottleneck is in the database layer. We're doing a full table scan on the products table which has about five million rows. The solution I'm proposing is one add an index on the name and description columns two implement search result caching with a thirty second TTL and three add pagination so we're not returning all results at once. I think with these three changes we can get the response time down to under two hundred milliseconds.",
     "The main issue we're seeing is that the response times for the search endpoint are way too slow. Right now it's taking about 3 to 4 seconds for a simple query and users are complaining. I did some profiling and it looks like the bottleneck is in the database layer. We're doing a full table scan on the products table which has about 5 million rows. The solution I'm proposing is:\n1. Add an index on the name and description columns\n2. Implement search result caching with a 30-second TTL\n3. Add pagination so we're not returning all results at once\n\nI think with these three changes we can get the response time down to under 200 milliseconds.",
     "Technical diagnosis with numbered solution"),
    ("long_03", "long_dictation",
     "For the new onboarding flow I'm thinking we should have like five screens. The first screen is the welcome screen where we just say hey welcome to the app. The second screen asks them to create their profile with their name and photo. The third screen is where they uh set their preferences like notification settings and theme choice. The fourth screen gives them a quick tutorial of the main features. And the fifth screen is a completion screen where we say you're all set and uh drop them into the main app. Each screen should have a next button and a skip button except for the last one which just has a get started button. Oh and we should also add a progress bar at the top so they know how far along they are.",
     "For the new onboarding flow, I'm thinking we should have five screens:\n1. Welcome screen — say welcome to the app\n2. Profile creation — name and photo\n3. Preferences — notification settings and theme choice\n4. Tutorial — quick overview of main features\n5. Completion — \"You're all set\" and drop into the main app\n\nEach screen should have a Next button and a Skip button, except for the last one which just has a Get Started button. We should also add a progress bar at the top so they know how far along they are.",
     "Feature description with numbered screens"),
    ("long_04", "long_dictation",
     "Alright uh so basically the the deployment failed last night and here's what happened. At around uh eleven PM the CI pipeline triggered the production deploy. The build succeeded but uh during the migration step the database uh threw a timeout error. The migration was trying to uh add a new column to the orders table which has about uh ten million rows and it locked the entire table. This caused all the API requests to queue up and eventually the the load balancer started returning uh five oh three errors. The uh the on-call engineer rolled back the deployment at about midnight but um some orders were lost during the twenty minute outage. Going forward we need to uh one use online schema migrations that don't lock tables two add a database health check to the deployment pipeline and three set up better alerting so we catch these issues faster.",
     "The deployment failed last night. Here's what happened: at around 11 PM, the CI pipeline triggered the production deploy. The build succeeded but during the migration step, the database threw a timeout error. The migration was trying to add a new column to the orders table which has about 10 million rows, and it locked the entire table. This caused all the API requests to queue up and eventually the load balancer started returning 503 errors. The on-call engineer rolled back the deployment at about midnight, but some orders were lost during the 20-minute outage. Going forward we need to:\n1. Use online schema migrations that don't lock tables\n2. Add a database health check to the deployment pipeline\n3. Set up better alerting so we catch these issues faster",
     "Incident report with root cause and action items"),
    ("long_05", "long_dictation",
     "I've been looking at our error monitoring dashboard and I'm seeing uh a pattern. Every day between uh two PM and four PM we get a spike in uh four twenty nine too many requests errors. Looking at the logs it seems like it's coming from one specific client that's hitting our API with about a thousand requests per second. They're basically scraping our product catalog. We should uh probably do three things about this. First add rate limiting at the API gateway level with a limit of maybe a hundred requests per minute per API key. Second reach out to this client and uh tell them about our bulk data export API which is designed for this kind of usage. And third add monitoring alerts for when any single client exceeds uh five hundred requests per minute so we catch this kind of thing earlier.",
     "I've been looking at our error monitoring dashboard and I'm seeing a pattern. Every day between 2 PM and 4 PM, we get a spike in 429 Too Many Requests errors. Looking at the logs, it seems like it's coming from one specific client that's hitting our API with about 1,000 requests per second. They're basically scraping our product catalog. We should probably do three things about this:\n1. Add rate limiting at the API gateway level with a limit of maybe 100 requests per minute per API key\n2. Reach out to this client and tell them about our bulk data export API which is designed for this kind of usage\n3. Add monitoring alerts for when any single client exceeds 500 requests per minute so we catch this kind of thing earlier",
     "Monitoring analysis with action items"),
]


def main():
    with open(OUTPUT, "w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["id", "category", "raw", "expected", "notes"])
        for row in SAMPLES:
            writer.writerow(row)

    print(f"Generated {len(SAMPLES)} samples → {OUTPUT}")

    # Print category breakdown
    from collections import Counter
    cats = Counter(row[1] for row in SAMPLES)
    print("\nCategory breakdown:")
    for cat, count in sorted(cats.items()):
        print(f"  {cat}: {count}")
    print(f"  TOTAL: {sum(cats.values())}")


if __name__ == "__main__":
    main()
