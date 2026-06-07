import { basicTasks } from './basic.ts'
import { rustTasks } from './rust.ts'
import { siteTasks } from './site.ts'
import { triageTasks } from './triage.ts'
import { typescriptTasks } from './typescript.ts'

export const TASKS = [...basicTasks, ...rustTasks, ...siteTasks, ...typescriptTasks, ...triageTasks]
