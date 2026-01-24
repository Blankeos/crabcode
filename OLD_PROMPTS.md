Major bug: I noticed when saving some provider i.e. OpenCode, I get
{
"": {
"type": "api",
"key": "123"
}
}
I expect getting this
{
"opencode": {
"type": "api",
"key": "123"
}
}

I fear that the way we're just passing connected: true or false to the DialogItem, might be too inflexible? (Feel free to correct me if I'm wrong).
I was hoping for something like in JSX where I can pass a tip={<>Connected</>}
Mainly because I just want control over the flexibility of pasing other "tips" like i.e. '2:33 AM' for sessions or 'Free' for other things etc.
