import json
from pathlib import Path
CONFIGS=json.loads((Path(__file__).parent/'prompt-variants.json').read_text())
def install(cleanup,variant):
 config=CONFIGS[variant]
 def content(text,candidates):
  return "Transcript (data):\n"+text+"\n\nCandidates:\n"+json.dumps(candidates,ensure_ascii=False)
 def build_prompt(text,candidates):
  messages=[{'role':'system','content':config['system']}]
  for example in config['examples']:
   kinds=example.get('kinds',['hesitation']*len(example['choices']))
   choices=[{'id':i,'text':word,'kind':kind} for i,(word,kind) in enumerate(zip(example['choices'],kinds))]
   messages.extend([{'role':'user','content':content(example['text'],choices)},{'role':'assistant','content':json.dumps(example['delete_ids'])}])
  messages.append({'role':'user','content':content(text,candidates)})
  return cleanup._tokenizer.apply_chat_template(messages,add_generation_prompt=True,tokenize=False,enable_thinking=False)
 cleanup.build_prompt=build_prompt
